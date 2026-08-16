//! V4L2 capture of the Infiray P2Pro thermal camera (Linux desktop).
//!
//! The camera exposes a single YUYV video node at 256x384: the top half
//! (192 rows) is the plain 8-bit video preview, the bottom half (192 rows)
//! is the raw thermal data, where every 2 bytes that would normally be a
//! YUYV luma/chroma pair are instead a little-endian 16-bit raw sample.

use super::{CaptureState, HEIGHT, WIDTH, decode_frame};
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

async fn probe_devices(to_ui: mpsc::Sender<CaptureState>) -> ah::Result<(V4lDevice, PathBuf)> {
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
                && let Ok(camera) = V4lDevice::new(&entry.path(), to_ui.clone())
            {
                println!("Found p2pro device: {}", entry.path().display());
                return Ok((camera, entry.path()));
            }
        }

        // Wait a bit before retrying.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

struct V4lDevice {
    device: Device,
    capabilities: Capabilities,
    fmt: Format,
    to_ui: mpsc::Sender<CaptureState>,
    renderer: Mutex<Renderer>,
}

impl V4lDevice {
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
            let frame = {
                let mut renderer = self.renderer.lock().expect("Lock poisoned");
                decode_frame(&mut renderer, buf, self.fmt.stride as usize)
            };
            if let Some(frame) = frame {
                let _ = self.to_ui.send(CaptureState::Frame(frame)).await;
            }
        }
    }
}

/// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
/// retrying on error (e.g. camera unplugged or not found yet).
pub(super) async fn capture_loop(device_path: Option<&Path>, to_ui: mpsc::Sender<CaptureState>) {
    loop {
        let camera = if let Some(device_path) = &device_path {
            // Open the specified device.
            match V4lDevice::new(device_path, to_ui.clone()) {
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
