//! Thermal camera capture: shared frame/state types, plus a platform-specific
//! capture backend.

use crate::render::Renderer;
use std::path::Path;
use tokio::sync::mpsc;

#[cfg(target_os = "linux")]
mod v4l;

#[cfg(target_os = "android")]
mod android;

/// Width of both the video and thermal half, in pixels.
pub const WIDTH: u32 = 256;
/// Height of the thermal-only half, in pixels (the full captured frame is
/// twice this, since it also contains the plain video half on top).
pub const HEIGHT: u32 = 192;

/// Shared state, signalled to the UI via a `tokio::sync::watch` channel and
/// written to by the capture thread.
#[derive(Clone)]
pub enum CaptureState {
    Connecting,
    Info(String),
    Error(String),
    Frame(ThermalFrame),
}

#[derive(Clone, PartialEq)]
pub struct ThermalFrame {
    pub png_bytes: Vec<u8>,
    pub png_uri: String,
    pub width: u32,
    pub height: u32,
    pub min_temp: f32,
    pub max_temp: f32,
    pub min_pos: (u32, u32),
    pub max_pos: (u32, u32),
}

pub struct Camera;

impl Camera {
    /// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
    /// retrying on error (e.g. camera unplugged or not found yet).
    ///
    /// `device_path` (an explicit `/dev/videoX` path) is only meaningful on
    /// the V4L2 (Linux desktop) backend; it is ignored on Android, which has
    /// no such device nodes.
    pub async fn capture_loop(device_path: Option<&Path>, to_ui: mpsc::Sender<CaptureState>) {
        #[cfg(target_os = "linux")]
        v4l::capture_loop(device_path, to_ui).await;

        #[cfg(target_os = "android")]
        {
            let _ = device_path;
            android::capture_loop(to_ui).await;
        }
    }
}

/// Decodes one raw YUYV buffer (full frame: video half on top, thermal half
/// on the bottom, see module docs) into a rendered [`ThermalFrame`], using
/// `renderer` for the false-color mapping. Returns `None` if the buffer is
/// short (a dropped/truncated frame - just skip it).
///
/// `stride` is the number of bytes per row (>= `WIDTH * 2`).
pub(crate) fn decode_frame(
    renderer: &mut Renderer,
    buf: &[u8],
    stride: usize,
) -> Option<ThermalFrame> {
    let half_height = HEIGHT as usize;
    let width = WIDTH as usize;

    let buf_len = buf.len();
    let min_buf_len = stride * half_height * 2;
    if buf_len < min_buf_len {
        eprintln!("Camera buffer too short: {buf_len} bytes (expected at least {min_buf_len})");
        return None;
    }

    let mut temps = Vec::with_capacity(width * half_height);
    for y in 0..half_height {
        let row = half_height + y; // bottom half carries the raw thermal data
        let row_start = row * stride;
        for x in 0..width {
            let offset = row_start + x * 2;
            let raw = buf[offset] as u16 | ((buf[offset + 1] as u16) << 8);
            temps.push(raw as f32 / 64.0 - 273.2); // raw/64 - 273.2 (Celsius)
        }
    }

    let rendered = renderer.build_frame(WIDTH, HEIGHT, &temps);

    Some(ThermalFrame {
        png_bytes: rendered.png_bytes,
        png_uri: rendered.png_uri,
        width: WIDTH,
        height: HEIGHT,
        min_temp: rendered.min_temp,
        max_temp: rendered.max_temp,
        min_pos: rendered.min_pos,
        max_pos: rendered.max_pos,
    })
}
