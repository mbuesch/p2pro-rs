//! V4L2 capture of the Infiray P2Pro thermal camera (Linux desktop).
//!
//! The camera exposes a single YUYV video node at 256x384: the top half
//! (192 rows) is the plain 8-bit video preview, the bottom half (192 rows)
//! is the raw thermal data, where every 2 bytes that would normally be a
//! YUYV luma/chroma pair are instead a little-endian 16-bit raw sample.

use super::{CaptureState, HEIGHT, WIDTH, decode_frame};
use crate::{
    app::FromUi,
    camera::{PRODUCT_ID, VENDOR_ID},
    camera_config::CameraConfig,
    render::Renderer,
};
use anyhow::{self as ah, Context as _, format_err as err};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tokio::sync::{mpsc, watch};
use v4l::{
    Device, Format, FourCC,
    buffer::Type,
    capability::{Capabilities, Flags},
    io::{mmap::Stream as MmapStream, traits::CaptureStream},
    video::Capture,
};

async fn probe_devices(
    to_ui: mpsc::Sender<CaptureState>,
    from_ui: watch::Receiver<FromUi>,
) -> ah::Result<(V4lDevice, PathBuf)> {
    println!("Probing for P2Pro device in /dev/video* ...");
    loop {
        let _ = to_ui
            .send(CaptureState::Info(
                "Probing for P2Pro device in /dev/video*\nPlug in your device now ...".to_string(),
            ))
            .await;

        let mut dir = tokio::fs::read_dir("/dev")
            .await
            .context("Read /dev directory failed")?;
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            if let Some(name) = name.to_str()
                && name.starts_with("video")
                && let Ok(camera) =
                    V4lDevice::new(&entry.path(), to_ui.clone(), from_ui.clone()).await
            {
                println!("Found P2Pro device: {}", entry.path().display());
                return Ok((camera, entry.path()));
            }
        }

        // Wait a bit before retrying.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn find_usb_device(busnum: u8, devnum: u8) -> ah::Result<rusb::Device<rusb::GlobalContext>> {
    for dev in rusb::devices()?.iter() {
        if dev.bus_number() == busnum && dev.address() == devnum {
            return Ok(dev);
        }
    }
    Err(err!("USB device {busnum}:{devnum} not found."))
}

struct V4lDevice {
    device: Device,
    #[allow(dead_code)]
    usb_device: rusb::Device<rusb::GlobalContext>,
    capabilities: Capabilities,
    fmt: Format,
    to_ui: mpsc::Sender<CaptureState>,
    renderer: Mutex<Renderer>,
    from_ui: watch::Receiver<FromUi>,
}

impl V4lDevice {
    async fn new(
        device_path: &Path,
        to_ui: mpsc::Sender<CaptureState>,
        from_ui: watch::Receiver<FromUi>,
    ) -> ah::Result<Self> {
        let device = Device::with_path(device_path)?;

        let caps = device.query_caps()?;

        if !caps.capabilities.contains(Flags::VIDEO_CAPTURE) {
            return Err(err!(
                "Device '{}' is not a video capture device",
                device_path.display()
            ));
        }

        let Some(node) = device_path.file_name() else {
            return Err(err!("Failed to get file name from device path"));
        };
        let iface_path = fs::canonicalize(
            Path::new("/sys/class/video4linux")
                .join(node)
                .join("device"),
        )?;
        let Some(usb_path) = iface_path.parent() else {
            return Err(err!("Failed to get USB path from interface path"));
        };

        let usb_idvendor = fs::read_to_string(usb_path.join("idVendor"))
            .context("Failed to read idVendor")?
            .trim()
            .to_owned();
        let usb_idproduct = fs::read_to_string(usb_path.join("idProduct"))
            .context("Failed to read idProduct")?
            .trim()
            .to_owned();
        let usb_busnum = fs::read_to_string(usb_path.join("busnum"))
            .context("Failed to read busnum")?
            .trim()
            .to_owned();
        let usb_devnum = fs::read_to_string(usb_path.join("devnum"))
            .context("Failed to read devnum")?
            .trim()
            .to_owned();

        let usb_idvendor: u16 =
            u16::from_str_radix(&usb_idvendor, 16).context("Failed to parse idVendor")?;
        let usb_idproduct: u16 =
            u16::from_str_radix(&usb_idproduct, 16).context("Failed to parse idProduct")?;
        let usb_busnum: u8 = usb_busnum.parse().context("Failed to parse busnum")?;
        let usb_devnum: u8 = usb_devnum.parse().context("Failed to parse devnum")?;

        if usb_idvendor != VENDOR_ID || usb_idproduct != PRODUCT_ID {
            return Err(err!(
                "Device {:04x}:{:04x} is not a P2Pro device (expected {:04x}:{:04x})",
                usb_idvendor,
                usb_idproduct,
                VENDOR_ID,
                PRODUCT_ID,
            ));
        }

        let usb_device =
            find_usb_device(usb_busnum, usb_devnum).context("Failed to find USB device")?;

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

        let usb_handle = usb_device.open().context("Failed to open USB device")?;
        let mut conf = CameraConfig::from_hw_access(usb_handle);
        let summary = conf
            .device_info_summary()
            .await
            .context("Failed to get device info summary")?;
        println!("Camera info:");
        for line in summary {
            println!("    {line}");
        }
        if let Err(e) = conf.set_default().await {
            eprintln!("Failed to set default configuration: {e}");
        }

        Ok(Self {
            device,
            usb_device,
            capabilities: caps,
            fmt,
            to_ui,
            renderer: Mutex::new(Renderer::new()),
            from_ui,
        })
    }

    async fn run_capture_loop(&self) -> ah::Result<()> {
        println!("Using device: {}", self.capabilities.bus);

        let mut stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)?;

        loop {
            let (buf, _meta) = stream.next()?;
            let frame = {
                let from_ui = self.from_ui.borrow().clone();
                let mut renderer = self.renderer.lock().expect("Lock poisoned");
                decode_frame(&mut renderer, buf, self.fmt.stride as usize, &from_ui)
            };
            if let Some(frame) = frame {
                let _ = self.to_ui.send(CaptureState::Frame(frame)).await;
            }
        }
    }
}

/// Runs forever: (re)connects to the camera and streams frames into `to_ui`,
/// retrying on error (e.g. camera unplugged or not found yet).
pub async fn capture_loop(
    device_path: Option<&Path>,
    to_ui: mpsc::Sender<CaptureState>,
    from_ui: watch::Receiver<FromUi>,
) {
    loop {
        let camera = if let Some(device_path) = &device_path {
            // Open the specified device.
            match V4lDevice::new(device_path, to_ui.clone(), from_ui.clone()).await {
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
            // Try to find a P2Pro camera device.
            match probe_devices(to_ui.clone(), from_ui.clone()).await {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = to_ui
                        .send(CaptureState::Error(format!(
                            "Error probing for P2Pro device:\n{e}\nGiving up.",
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
