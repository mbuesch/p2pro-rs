//! V4L2 capture of the Infiray P2Pro thermal camera.
//!
//! The camera exposes a single YUYV video node at 256x384: the top half
//! (192 rows) is the plain 8-bit video preview, the bottom half (192 rows)
//! is the raw thermal data, where every 2 bytes that would normally be a
//! YUYV luma/chroma pair are instead a little-endian 16-bit raw sample.

use crate::render::Renderer;
use std::{io, time::Duration};
use tokio::sync::watch;
use v4l::{
    Device, Format, FourCC,
    buffer::Type,
    io::{mmap::Stream as MmapStream, traits::CaptureStream},
    video::Capture,
};

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
    Error(String),
    Frame(ThermalFrame),
}

#[derive(Clone, PartialEq)]
pub struct ThermalFrame {
    pub data_uri: String,
    pub width: u32,
    pub height: u32,
    pub min_temp: f32,
    pub max_temp: f32,
    pub min_pos: (u32, u32),
    pub max_pos: (u32, u32),
}

pub struct Camera {
    device_path: String,
    to_ui: watch::Sender<CaptureState>,
    renderer: Renderer,
}

impl Camera {
    /// Runs forever on a dedicated OS thread: (re)connects to the camera and
    /// streams frames into `to_ui`, retrying every couple seconds on error
    /// (e.g. camera unplugged or not found yet).
    pub fn capture_loop(device_path: String, to_ui: watch::Sender<CaptureState>) {
        let camera = Camera::new(device_path.clone(), to_ui.clone());
        loop {
            if let Err(e) = camera.run_session() {
                let _ = to_ui.send(CaptureState::Error(format!(
                    "{device_path}: {e} (retrying...)"
                )));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn new(device_path: String, to_ui: watch::Sender<CaptureState>) -> Self {
        Self {
            device_path,
            to_ui,
            renderer: Renderer::new(),
        }
    }

    fn run_session(&self) -> io::Result<()> {
        let dev = Device::with_path(&self.device_path)?;

        let requested = Format::new(WIDTH, HEIGHT * 2, FourCC::new(b"YUYV"));
        let fmt = dev.set_format(&requested)?;
        if fmt.width != WIDTH || fmt.height != HEIGHT * 2 {
            return Err(io::Error::other(format!(
                "camera reported an unexpected format {}x{} (wanted {}x{})",
                fmt.width,
                fmt.height,
                WIDTH,
                HEIGHT * 2
            )));
        }

        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, 4)?;

        loop {
            let (buf, _meta) = stream.next()?;
            if let Some(frame) = self.decode_frame(buf, &fmt) {
                let _ = self.to_ui.send(CaptureState::Frame(frame));
            }
        }
    }

    /// Decodes one raw V4L2 buffer into a rendered [`ThermalFrame`], or `None`
    /// if the buffer is short (a dropped/truncated frame - just skip it).
    fn decode_frame(&self, buf: &[u8], fmt: &Format) -> Option<ThermalFrame> {
        let stride = fmt.stride as usize;
        let half_height = HEIGHT as usize;
        let width = WIDTH as usize;

        if buf.len() < stride * fmt.height as usize {
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

        let rendered = self.renderer.build_frame(WIDTH, HEIGHT, &temps);
        Some(ThermalFrame {
            data_uri: rendered.data_uri,
            width: WIDTH,
            height: HEIGHT,
            min_temp: rendered.min_temp,
            max_temp: rendered.max_temp,
            min_pos: rendered.min_pos,
            max_pos: rendered.max_pos,
        })
    }
}
