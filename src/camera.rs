//! V4L2 capture of the Infiray P2Pro thermal camera.
//!
//! The camera exposes a single YUYV video node at 256x384: the top half
//! (192 rows) is the plain 8-bit video preview, the bottom half (192 rows)
//! is the raw thermal data, where every 2 bytes that would normally be a
//! YUYV luma/chroma pair are instead a little-endian 16-bit raw sample.

use crate::render::Renderer;
use anyhow::{self as ah, Context as _, format_err as err};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tokio::sync::mpsc;
use v4l::{
    Device, Format, FourCC,
    buffer::Type,
    capability::{Capabilities, Flags},
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
    Info(String),
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

async fn probe_devices(to_ui: mpsc::Sender<CaptureState>) -> ah::Result<(Camera, PathBuf)> {
    println!("Probing for p2pro device in /dev/video* ...");
    loop {
        let _ = to_ui
            .send(CaptureState::Info(
                "Probing for p2pro device in /dev/video*\nPlug in your device now ...".to_string(),
            ))
            .await;

        let mut dir = tokio::fs::read_dir("/dev")
            .await
            .context("Read /dev directory failed")?;
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            if let Some(name) = name.to_str()
                && name.starts_with("video")
                && let Ok(camera) = Camera::new(&entry.path(), to_ui.clone())
            {
                println!("Found p2pro device: {}", entry.path().display());
                return Ok((camera, entry.path()));
            }
        }

        // Wait a bit before retrying.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub struct Camera {
    device: Device,
    capabilities: Capabilities,
    fmt: Format,
    to_ui: mpsc::Sender<CaptureState>,
    renderer: Mutex<Renderer>,
}

impl Camera {
    /// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
    /// retrying on error (e.g. camera unplugged or not found yet).
    pub async fn capture_loop(device_path: Option<&Path>, to_ui: mpsc::Sender<CaptureState>) {
        loop {
            let camera = if let Some(device_path) = &device_path {
                // Open the specified device.
                match Camera::new(device_path, to_ui.clone()) {
                    Ok(c) => Some((c, device_path.to_path_buf())),
                    Err(e) => {
                        let _ = to_ui
                            .send(CaptureState::Error(format!(
                                "Error opening camera {}:\n{e}\nRetrying ...",
                                device_path.display()
                            )))
                            .await;
                        // Try again.
                        None
                    }
                }
            } else {
                // Try to find a p2pro camera device.
                match probe_devices(to_ui.clone()).await {
                    Ok(c) => Some(c),
                    Err(e) => {
                        let _ = to_ui
                            .send(CaptureState::Error(format!(
                                "Error probing for p2pro device:\n{e}\nGiving up.",
                            )))
                            .await;
                        // Do not try again. If probing fails then it's fatal.
                        return;
                    }
                }
            };

            // Run the capture loop.
            if let Some((camera, device_path)) = camera
                && let Err(e) = camera.run_capture_loop().await
            {
                let device_path = device_path.display();
                let _ = to_ui
                    .send(CaptureState::Error(format!(
                        "{device_path}:\n{e}\nRetrying ..."
                    )))
                    .await;
            }

            // Wait a bit before retrying.
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    fn new(device_path: &Path, to_ui: mpsc::Sender<CaptureState>) -> ah::Result<Self> {
        let device = Device::with_path(device_path)?;

        let caps = device.query_caps()?;

        if !caps.capabilities.contains(Flags::VIDEO_CAPTURE) {
            return Err(err!(
                "Device '{}' is not a video capture device",
                device_path.display()
            ));
        }

        let requested = Format::new(WIDTH, HEIGHT * 2, FourCC::new(b"YUYV"));
        let fmt = device.set_format(&requested)?;
        if fmt.width != requested.width
            || fmt.height != requested.height
            || fmt.fourcc != requested.fourcc
        {
            return Err(err!(
                "Camera reported an unexpected format {}x{}/{} (wanted {}x{}/{})",
                fmt.width,
                fmt.height,
                fmt.fourcc,
                requested.width,
                requested.height,
                requested.fourcc
            ));
        }

        Ok(Self {
            device,
            capabilities: caps,
            fmt,
            to_ui,
            renderer: Mutex::new(Renderer::new()),
        })
    }

    async fn run_capture_loop(&self) -> ah::Result<()> {
        println!("Using device: {}", self.capabilities.bus);

        let mut stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)?;

        loop {
            let (buf, _meta) = stream.next()?;
            if let Some(frame) = self.decode_frame(buf, &self.fmt) {
                let _ = self.to_ui.send(CaptureState::Frame(frame)).await;
            }
        }
    }

    /// Decodes one raw V4L2 buffer into a rendered [`ThermalFrame`], or `None`
    /// if the buffer is short (a dropped/truncated frame - just skip it).
    fn decode_frame(&self, buf: &[u8], fmt: &Format) -> Option<ThermalFrame> {
        let stride = fmt.stride as usize;
        let half_height = HEIGHT as usize;
        let width = WIDTH as usize;

        let buf_len = buf.len();
        let min_buf_len = stride * fmt.height as usize;
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

        let rendered = self
            .renderer
            .lock()
            .expect("Lock poisoned")
            .build_frame(WIDTH, HEIGHT, &temps);

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
