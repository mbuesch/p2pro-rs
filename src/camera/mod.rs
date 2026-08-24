//! Thermal camera capture: shared frame/state types, plus a platform-specific
//! capture backend.

use crate::render::{RenderedFrame, Renderer};
use std::path::Path;
use tokio::sync::mpsc;

#[cfg(target_os = "linux")]
mod v4l;

#[cfg(target_os = "android")]
pub mod android;

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
    Frame(RenderedFrame),
}

pub struct Camera;

impl Camera {
    /// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
    /// retrying on error (e.g. camera unplugged or not found yet).
    ///
    /// `device_path` (an explicit `/dev/videoX` path) is only meaningful on
    /// the V4L2 (Linux desktop) backend; it must be None on Android.
    pub async fn capture_loop(device_path: Option<&Path>, to_ui: mpsc::Sender<CaptureState>) {
        #[cfg(target_os = "linux")]
        v4l::capture_loop(device_path, to_ui).await;

        #[cfg(target_os = "android")]
        {
            assert!(device_path.is_none());
            android::capture_loop(to_ui).await;
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
) -> Option<RenderedFrame> {
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

    Some(rendered)
}
