//! USB Video Class (UVC) 1.1 descriptor parsing and Probe/Commit
//! negotiation for the P2Pro's VideoStreaming interface.
//!
//! This only implements the small slice of UVC needed for P2Pro.

use crate::camera::{HEIGHT, WIDTH};
use anyhow::{self as ah, Context as _, format_err as err};
use rusb::{
    Context, DeviceHandle, Direction, Error, Recipient, RequestType, TransferType, request_type,
};
use std::time::Duration;

const CC_VIDEO: u8 = 0x0e;
const SC_VIDEOSTREAMING: u8 = 0x02;
const CS_INTERFACE: u8 = 0x24;
const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
const VS_FRAME_UNCOMPRESSED: u8 = 0x05;

const VS_PROBE_CONTROL: u16 = 0x01;
const VS_COMMIT_CONTROL: u16 = 0x02;
const REQ_SET_CUR: u8 = 0x01;
const REQ_GET_CUR: u8 = 0x81;

const CONTROL_TIMEOUT: Duration = Duration::from_millis(1000);

/// The `MEDIASUBTYPE_YUY2` / UVC uncompressed-format GUID for "YUY2"
/// (a.k.a. YUYV): the fourCC bytes followed by the fixed UVC GUID suffix.
const YUY2_GUID: [u8; 16] = [
    0x59, 0x55, 0x59, 0x32, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// The result of a successful Probe/Commit negotiation.
pub struct Negotiated {
    pub endpoint: u8,
    pub transfer_type: TransferType,
    /// Size of one isochronous packet (only meaningful when `transfer_type == Isochronous`).
    pub packet_size: usize,
    pub max_payload_transfer_size: u32,
    pub max_video_frame_size: u32,
}

/// UVC "Video Probe and Commit Controls" structure (UVC 1.1, 26 bytes).
#[derive(Debug, Clone, Copy, Default)]
struct StreamingControl {
    bm_hint: u16,
    format_index: u8,
    frame_index: u8,
    frame_interval: u32,
    key_frame_rate: u16,
    p_frame_rate: u16,
    comp_quality: u16,
    comp_window_size: u16,
    delay: u16,
    max_video_frame_size: u32,
    max_payload_transfer_size: u32,
}

impl StreamingControl {
    const LEN: usize = 26;

    fn to_bytes(self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0..2].copy_from_slice(&self.bm_hint.to_le_bytes());
        b[2] = self.format_index;
        b[3] = self.frame_index;
        b[4..8].copy_from_slice(&self.frame_interval.to_le_bytes());
        b[8..10].copy_from_slice(&self.key_frame_rate.to_le_bytes());
        b[10..12].copy_from_slice(&self.p_frame_rate.to_le_bytes());
        b[12..14].copy_from_slice(&self.comp_quality.to_le_bytes());
        b[14..16].copy_from_slice(&self.comp_window_size.to_le_bytes());
        b[16..18].copy_from_slice(&self.delay.to_le_bytes());
        b[18..22].copy_from_slice(&self.max_video_frame_size.to_le_bytes());
        b[22..26].copy_from_slice(&self.max_payload_transfer_size.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::LEN {
            return None;
        }
        Some(Self {
            bm_hint: u16::from_le_bytes([b[0], b[1]]),
            format_index: b[2],
            frame_index: b[3],
            frame_interval: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            key_frame_rate: u16::from_le_bytes([b[8], b[9]]),
            p_frame_rate: u16::from_le_bytes([b[10], b[11]]),
            comp_quality: u16::from_le_bytes([b[12], b[13]]),
            comp_window_size: u16::from_le_bytes([b[14], b[15]]),
            delay: u16::from_le_bytes([b[16], b[17]]),
            max_video_frame_size: u32::from_le_bytes([b[18], b[19], b[20], b[21]]),
            max_payload_transfer_size: u32::from_le_bytes([b[22], b[23], b[24], b[25]]),
        })
    }
}

/// Negotiates a YUYV stream at `WIDTH`x`HEIGHT*2` with the P2Pro and leaves
/// its VideoStreaming interface set to the chosen alternate setting, ready
/// for reading off `Negotiated::endpoint`.
pub fn negotiate(handle: &DeviceHandle<Context>) -> ah::Result<Negotiated> {
    let device = handle.device();
    let config = device
        .active_config_descriptor()
        .context("Failed to read the active USB configuration descriptor")?;

    let mut vs_interface_number = None;
    let mut probe = StreamingControl::default();

    'a: for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if alt.class_code() != CC_VIDEO || alt.sub_class_code() != SC_VIDEOSTREAMING {
                continue;
            }
            let Some((format_index, frame_index, frame_interval)) =
                parse_vs_descriptors(alt.extra())
            else {
                continue;
            };
            vs_interface_number = Some(alt.interface_number());
            probe.format_index = format_index;
            probe.frame_index = frame_index;
            probe.frame_interval = frame_interval;
            break 'a;
        }
    }
    let vs_interface_number = vs_interface_number.ok_or_else(|| {
        err!(
            "Could not find a VideoStreaming interface offering YUYV at {}x{}",
            WIDTH,
            HEIGHT * 2
        )
    })?;

    // Detach the driver, if any.
    match handle.kernel_driver_active(vs_interface_number) {
        Ok(true) => handle.detach_kernel_driver(vs_interface_number).context(
            "Failed to detach the kernel/usbfs driver from the VideoStreaming interface",
        )?,
        Ok(false) => {}
        Err(Error::NotSupported | Error::NotFound) => {}
        Err(e) => return Err(e).context("Failed to query the VideoStreaming kernel driver state"),
    }

    handle
        .claim_interface(vs_interface_number)
        .context("Failed to claim the VideoStreaming interface")?;

    set_cur(handle, vs_interface_number, VS_PROBE_CONTROL, &probe)
        .context("VS_PROBE_CONTROL (SET_CUR) failed")?;
    if let Ok(readback) = get_cur(handle, vs_interface_number, VS_PROBE_CONTROL) {
        probe = readback;
    }
    set_cur(handle, vs_interface_number, VS_COMMIT_CONTROL, &probe)
        .context("VS_COMMIT_CONTROL (SET_CUR) failed")?;

    let required = (probe.max_payload_transfer_size as usize).max(1);
    let mut chosen: Option<(u8, u8, TransferType, usize)> = None;
    for interface in config.interfaces() {
        if interface.number() != vs_interface_number {
            continue;
        }
        for alt in interface.descriptors() {
            if alt.setting_number() == 0 {
                continue; // the zero-bandwidth idle setting
            }
            let Some(ep) = alt
                .endpoint_descriptors()
                .find(|e| e.direction() == Direction::In)
            else {
                continue;
            };
            let transfer_type = ep.transfer_type();
            if !matches!(
                transfer_type,
                TransferType::Isochronous | TransferType::Bulk
            ) {
                continue;
            }
            let size = packet_size_bytes(ep.transfer_type(), ep.max_packet_size());
            let candidate = (alt.setting_number(), ep.address(), transfer_type, size);
            let is_better = match chosen {
                None => true,
                Some((_, _, _, best_size)) => match (size >= required, best_size >= required) {
                    (true, false) => true,
                    (true, true) => size < best_size,
                    (false, true) => false,
                    (false, false) => size > best_size,
                },
            };
            if is_better {
                chosen = Some(candidate);
            }
        }
    }
    let (alt_setting, endpoint, transfer_type, packet_size) =
        chosen.ok_or_else(|| err!("No usable VideoStreaming alternate setting/endpoint found"))?;

    handle
        .set_alternate_setting(vs_interface_number, alt_setting)
        .context("Failed to select the VideoStreaming alternate setting")?;

    Ok(Negotiated {
        endpoint,
        transfer_type,
        packet_size,
        max_payload_transfer_size: probe.max_payload_transfer_size,
        max_video_frame_size: probe.max_video_frame_size,
    })
}

/// Scans one VideoStreaming alternate setting's class-specific descriptors
/// for a YUY2 entry and a matching entry for our target resolution.
///
/// Returns `(format_index, frame_index, default_frame_interval)`.
fn parse_vs_descriptors(extra: &[u8]) -> Option<(u8, u8, u32)> {
    let mut format_index = None;
    let mut pos = 0;
    let mut result = None;
    while pos + 3 <= extra.len() {
        let len = extra[pos] as usize;
        if len < 3 || pos + len > extra.len() {
            break;
        }
        let descriptor_type = extra[pos + 1];
        let subtype = extra[pos + 2];
        if descriptor_type == CS_INTERFACE {
            match subtype {
                VS_FORMAT_UNCOMPRESSED if len >= 27 => {
                    format_index =
                        (extra[pos + 5..pos + 21] == YUY2_GUID).then_some(extra[pos + 3]);
                }
                VS_FRAME_UNCOMPRESSED if len >= 25 => {
                    if let Some(format_index) = format_index {
                        let width = u16::from_le_bytes([extra[pos + 5], extra[pos + 6]]);
                        let height = u16::from_le_bytes([extra[pos + 7], extra[pos + 8]]);
                        if width as u32 == WIDTH && height as u32 == HEIGHT * 2 {
                            let frame_index = extra[pos + 3];
                            let default_interval = u32::from_le_bytes([
                                extra[pos + 21],
                                extra[pos + 22],
                                extra[pos + 23],
                                extra[pos + 24],
                            ]);
                            result = Some((format_index, frame_index, default_interval));
                        }
                    }
                }
                _ => {}
            }
        }
        pos += len;
    }
    result
}

/// Decodes an endpoint's `wMaxPacketSize` into the actual number of bytes transferable per frame.
fn packet_size_bytes(transfer_type: TransferType, raw: u16) -> usize {
    match transfer_type {
        TransferType::Isochronous | TransferType::Interrupt => {
            let base = (raw & 0x07ff) as usize;
            let extra_transactions = ((raw >> 11) & 0x3) as usize;
            base * (extra_transactions + 1)
        }
        _ => raw as usize,
    }
}

fn set_cur(
    handle: &DeviceHandle<Context>,
    vs_interface: u8,
    selector: u16,
    ctrl: &StreamingControl,
) -> ah::Result<()> {
    let bytes = ctrl.to_bytes();
    handle.write_control(
        request_type(Direction::Out, RequestType::Class, Recipient::Interface),
        REQ_SET_CUR,
        selector << 8,
        vs_interface as u16,
        &bytes,
        CONTROL_TIMEOUT,
    )?;
    Ok(())
}

fn get_cur(
    handle: &DeviceHandle<Context>,
    vs_interface: u8,
    selector: u16,
) -> ah::Result<StreamingControl> {
    let mut buf = [0u8; StreamingControl::LEN];
    handle.read_control(
        request_type(Direction::In, RequestType::Class, Recipient::Interface),
        REQ_GET_CUR,
        selector << 8,
        vs_interface as u16,
        &mut buf,
        CONTROL_TIMEOUT,
    )?;
    StreamingControl::from_bytes(&buf).ok_or_else(|| err!("Short GET_CUR response"))
}
