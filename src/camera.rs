//! Thermal camera capture: shared frame/state types, plus a platform-specific
//! capture backend.

use crate::{
    app::FromUi,
    render::{RenderedFrame, Renderer},
    util::FastFloat as _,
};
use anyhow as ah;
use p2pro_hw::{CameraConfig, CameraConfigHwAccess};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc, watch},
    time::interval,
};

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub mod uvc;

mod dummy;

#[cfg(target_os = "linux")]
mod v4l;

/// Width of both the video and thermal half, in pixels.
pub const WIDTH: u32 = 256;
/// Height of the thermal-only half, in pixels.
pub const HEIGHT: u32 = 192;

/// InfiRay P2Pro USB vendor ID.
pub const VENDOR_ID: u16 = 0x0bda;
/// InfiRay P2Pro USB product ID.
pub const PRODUCT_ID: u16 = 0x5830;

/// Shared state between capture loop and UI.
#[derive(Clone)]
pub enum CaptureState {
    Connecting,
    Info(String),
    Error(String),
    Telemetry(CameraTelemetry),
    Frame(RenderedFrame),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraTelemetry {
    pub shutter_vtemp: u16,
    pub current_vtemp: u16,
    pub reflected_temperature: f32,
    pub atmospheric_temperature: f32,
    pub emissivity: f32,
    pub atmospheric_transmittance: f32,
}

pub struct Camera;

impl Camera {
    /// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
    /// retrying on error (e.g. camera unplugged or not found yet).
    ///
    /// `device_path` (an explicit `/dev/videoX` path) is only meaningful on
    /// the V4L2 (Linux desktop) backend; it must be None on Android.
    ///
    /// When `demo` is true, no hardware is used: the dummy backend generates
    /// an animated test picture instead.
    pub async fn capture_loop(
        device_path: Option<&Path>,
        demo: bool,
        to_ui: mpsc::Sender<CaptureState>,
        from_ui: watch::Receiver<FromUi>,
    ) {
        if demo {
            dummy::capture_loop(to_ui, from_ui).await;
        } else {
            #[cfg(target_os = "linux")]
            v4l::capture_loop(device_path, to_ui, from_ui).await;

            #[cfg(target_os = "android")]
            {
                assert!(device_path.is_none());
                android::capture_loop(to_ui, from_ui).await;
            }
        }
    }
}

/// Applies config changes from the UI while this USB camera session is active.
pub async fn apply_config_updates<H>(
    usb_handle: Arc<AsyncMutex<H>>,
    mut from_ui: watch::Receiver<FromUi>,
    to_ui: mpsc::Sender<CaptureState>,
) where
    H: CameraConfigHwAccess + Send + 'static,
{
    let mut config = CameraConfig::from_hw_access(usb_handle);

    let mut prev_high_gain = None;
    let mut prev_emissivity = None;
    let mut prev_atmospheric_transmittance = None;
    let mut prev_atmospheric_temperature = None;
    let mut prev_reflected_temperature = None;
    let mut prev_distance = None;

    let mut read_interval = interval(Duration::from_secs(1));

    loop {
        let settings = from_ui.borrow_and_update().clone();

        if prev_high_gain != settings.high_gain
            && let Some(high_gain) = settings.high_gain
        {
            match config.set_high_gain(high_gain).await {
                Ok(()) => prev_high_gain = Some(high_gain),
                Err(err) => log::error!("Failed to set high-gain mode: {err}"),
            }
        }

        if prev_emissivity != settings.emissivity
            && let Some(emissivity) = settings.emissivity
        {
            match config.set_emissivity(emissivity).await {
                Ok(()) => prev_emissivity = Some(emissivity),
                Err(err) => log::error!("Failed to set emissivity: {err}"),
            }
        }

        if prev_atmospheric_transmittance != settings.atmospheric_transmittance
            && let Some(transmittance) = settings.atmospheric_transmittance
        {
            match config.set_atmospheric_transmittance(transmittance).await {
                Ok(()) => prev_atmospheric_transmittance = Some(transmittance),
                Err(err) => log::error!("Failed to set atmospheric transmittance: {err}"),
            }
        }

        if prev_atmospheric_temperature != settings.atmospheric_temperature
            && let Some(temperature) = settings.atmospheric_temperature
        {
            match config.set_atmospheric_temperature(temperature).await {
                Ok(()) => prev_atmospheric_temperature = Some(temperature),
                Err(err) => log::error!("Failed to set atmospheric temperature: {err}"),
            }
        }

        if prev_reflected_temperature != settings.reflected_temperature
            && let Some(temperature) = settings.reflected_temperature
        {
            match config.set_reflected_temperature(temperature).await {
                Ok(()) => prev_reflected_temperature = Some(temperature),
                Err(err) => log::error!("Failed to set reflected temperature: {err}"),
            }
        }

        if prev_distance != settings.distance
            && let Some(distance) = settings.distance
        {
            match config.set_distance(distance).await {
                Ok(()) => prev_distance = Some(distance),
                Err(err) => log::error!("Failed to set object distance: {err}"),
            }
        }

        tokio::select! {
            biased;
            _ = read_interval.tick() => {
                let telemetry: ah::Result<_> = async {
                    let telemetry = CameraTelemetry {
                        shutter_vtemp: config.shutter_vtemp().await?,
                        current_vtemp: config.current_vtemp().await?,
                        reflected_temperature: config.reflected_temperature().await?,
                        atmospheric_temperature: config.atmospheric_temperature().await?,
                        emissivity: config.emissivity().await?,
                        atmospheric_transmittance: config.atmospheric_transmittance().await?,
                    };
                    Ok(telemetry)
                }
                .await;
                match telemetry {
                    Ok(telemetry) => {
                        let _ = to_ui.try_send(CaptureState::Telemetry(telemetry));
                    }
                    Err(err) => log::error!("Failed to read camera telemetry: {err}"),
                }
            }
            changed = from_ui.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }
}

/// Decodes one raw YUYV buffer into a [`RenderedFrame`].
/// Full frame: Video half on top, thermal half on the bottom.
///
/// `stride` is the number of bytes per row (>= `WIDTH * 2`).
pub fn decode_frame(
    renderer: &mut Renderer,
    buf: &[u8],
    stride: usize,
    from_ui: &FromUi,
) -> Option<RenderedFrame> {
    let half_height = HEIGHT as usize;
    let width = WIDTH as usize;

    let buf_len = buf.len();
    let min_buf_len = stride * half_height * 2;
    if buf_len < min_buf_len {
        eprintln!("Camera buffer too short: {buf_len} bytes (expected at least {min_buf_len})");
        return None;
    }

    // Convert camera image to temperature pixels.
    let mut temps = Vec::with_capacity(width * half_height);
    for y in 0..half_height {
        let row = half_height + y; // bottom half carries the raw thermal data
        let row_start = row * stride;
        assert!(row_start + ((width - 1) * 2) + 1 < buf.len());
        for x in 0..width {
            let offset = row_start + (x * 2);
            let raw = buf[offset] as u16 | ((buf[offset + 1] as u16) << 8);
            let raw = raw as f32;
            temps.push((raw.fdiv(64.0)).fsub(273.15)); // raw/64 - 273.15 (Celsius)
        }
    }

    Some(renderer.build_frame(WIDTH, HEIGHT, &temps, from_ui))
}
