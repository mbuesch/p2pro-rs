//! Native Android capture backend.
//!
//! Android has no V4L2 access to arbitrary USB UVC devices.
//! Instead this talks to the P2Pro directly over USB as a userspace UVC driver using `libusb`/`rusb`.

mod jni_bridge;
mod protocol;
mod stream;

use super::CaptureState;
use anyhow::{self as ah, Context as _};
use jni_bridge::{SessionGuard, UsbEvent};
use rusb::UsbContext;
use std::{collections::VecDeque, os::fd::RawFd, time::Duration};
use tokio::sync::mpsc;

/// InfiRay P2Pro USB vendor/product ID.
const VENDOR_ID: u16 = 0x0bda;
const PRODUCT_ID: u16 = 0x5830;

/// Bounded ring buffer of log lines that are shown on screen.
struct DebugLog {
    lines: VecDeque<String>,
}

impl DebugLog {
    /// Maximum number of debug lines kept for the on-screen debug log.
    const MAX_LINES: usize = 16;

    fn new() -> Self {
        Self {
            lines: VecDeque::new(),
        }
    }

    fn push(&mut self, line: impl Into<String>) {
        let line = line.into();
        eprintln!("P2Pro: {}", line);
        if self.lines.len() >= Self::MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }
}

fn waiting_message(log: &DebugLog) -> String {
    format!(
        "Waiting for the P2Pro to be plugged in ...\n\
         Grant the USB and camera permission popups when they appear.\n\n\
         {}",
        log.render()
    )
}

/// Main capture loop.
pub async fn capture_loop(to_ui: mpsc::Sender<CaptureState>) {
    let mut log = DebugLog::new();
    loop {
        let _ = to_ui.send(CaptureState::Info(waiting_message(&log))).await;

        match jni_bridge::next_event().await {
            UsbEvent::Log(line) => {
                log::info!("Kotlin: {line}");
                log.push(line);
            }
            UsbEvent::DeviceReady(fd, vendor_id, product_id, token) => {
                log.push(format!(
                    "Rust: received fd {fd} for {vendor_id:04x}:{product_id:04x}"
                ));
                if vendor_id != VENDOR_ID || product_id != PRODUCT_ID {
                    log.push(format!(
                        "Not a P2Pro (expected {VENDOR_ID:04x}:{PRODUCT_ID:04x}); ignoring fd"
                    ));
                    continue;
                }
                let _ = to_ui.send(CaptureState::Info(waiting_message(&log))).await;
                if let Err(e) = run_session(fd, token, to_ui.clone()).await {
                    log::error!("P2Pro USB session failed: {e:#}");
                    log.push(format!("Session error: {e:#}"));
                    let _ = to_ui
                        .send(CaptureState::Error(format!(
                            "{e:#}\nRetrying ...\n\n{}",
                            log.render()
                        )))
                        .await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

async fn run_session(fd: RawFd, token: i64, to_ui: mpsc::Sender<CaptureState>) -> ah::Result<()> {
    match tokio::task::spawn_blocking(move || run_session_blocking(fd, token, to_ui)).await {
        Ok(result) => result,
        Err(join_err) => Err(ah::Error::new(join_err).context("USB capture thread panicked")),
    }
}

/// USB Video Class session (negotiation + streaming).
fn run_session_blocking(
    fd: RawFd,
    token: i64,
    to_ui: mpsc::Sender<CaptureState>,
) -> ah::Result<()> {
    let _session_guard = SessionGuard::new(token);

    let context = rusb::Context::new().context("Failed to create a libusb context")?;

    // SAFETY: `fd` is a USB device file descriptor already opened.
    let handle = unsafe { context.open_device_with_fd(fd) }
        .context("Failed to wrap the Android USB file descriptor")?;
    let _ = to_ui.blocking_send(CaptureState::Info(
        "USB device opened via libusb; negotiating UVC format ...".to_string(),
    ));

    let negotiated = protocol::negotiate(&handle)
        .context("USB Video Class negotiation with the P2Pro failed")?;
    log::info!(
        "P2Pro: streaming over USB {:?} endpoint 0x{:02x} ({} bytes/payload, {} bytes/frame)",
        negotiated.transfer_type,
        negotiated.endpoint,
        negotiated.max_payload_transfer_size,
        negotiated.max_video_frame_size,
    );
    let _ = to_ui.blocking_send(CaptureState::Info(format!(
        "UVC negotiated: {:?} endpoint 0x{:02x} ({} bytes/payload)\n\
         Waiting for frames ...",
        negotiated.transfer_type, negotiated.endpoint, negotiated.max_payload_transfer_size,
    )));

    stream::run(&handle, &negotiated, to_ui)
}
