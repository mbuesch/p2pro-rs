//! Reads UVC payload data off the negotiated endpoint (bulk or isochronous)
//! and reassembles it into complete thermal frames.

use super::protocol::Negotiated;
use crate::{
    camera::{CaptureState, HEIGHT, WIDTH, decode_frame},
    render::Renderer,
};
use anyhow::{self as ah, format_err as err};
use rusb::{Context, DeviceHandle, TransferType, UsbContext, ffi};
use std::{ffi::c_void, time::Duration};
use tokio::sync::mpsc;

/// Full raw YUYV frame size: video half on top, thermal half on the bottom.
const FRAME_BYTES: usize = WIDTH as usize * 2 * (HEIGHT as usize * 2);

const BULK_TIMEOUT: Duration = Duration::from_secs(2);
const ISO_TRANSFERS: usize = 4;
const ISO_PACKETS_PER_TRANSFER: usize = 32;

pub fn run(
    handle: &DeviceHandle<Context>,
    negotiated: &Negotiated,
    to_ui: mpsc::Sender<CaptureState>,
) -> ah::Result<()> {
    let mut collector = Collector {
        reassembler: FrameReassembler::new(FRAME_BYTES),
        renderer: Renderer::new(),
        to_ui,
    };

    match negotiated.transfer_type {
        TransferType::Bulk => run_bulk(handle, negotiated, &mut collector),
        TransferType::Isochronous => run_iso(handle, negotiated, &mut collector),
        other => Err(err!(
            "Unsupported UVC video endpoint transfer type: {other:?}"
        )),
    }
}

/// Accumulates UVC payloads into frames and forwards decoded frames to the UI.
struct Collector {
    reassembler: FrameReassembler,
    renderer: Renderer,
    to_ui: mpsc::Sender<CaptureState>,
}

impl Collector {
    fn feed_payload(&mut self, chunk: &[u8]) {
        if let Some(frame_bytes) = self.reassembler.feed(chunk)
            && let Some(frame) = decode_frame(&mut self.renderer, &frame_bytes, WIDTH as usize * 2)
        {
            let _ = self.to_ui.blocking_send(CaptureState::Frame(frame));
        }
    }
}

/// Strips UVC stream payload headers and glues payloads back together into
/// complete frames, using the header's FID (frame-toggle) and EOF bits.
struct FrameReassembler {
    buf: Vec<u8>,
    last_fid: Option<bool>,
    max_frame_size: usize,
}

impl FrameReassembler {
    fn new(max_frame_size: usize) -> Self {
        Self {
            buf: Vec::with_capacity(max_frame_size),
            last_fid: None,
            max_frame_size,
        }
    }

    /// Feeds one payload chunk (UVC payload header included). Returns
    /// `Some(bytes)` once a complete frame has been assembled.
    fn feed(&mut self, chunk: &[u8]) -> Option<Vec<u8>> {
        let (fid, eof, data) = parse_payload_header(chunk)?;

        if let Some(last_fid) = self.last_fid
            && last_fid != fid
            && !self.buf.is_empty()
        {
            // The previous frame never got an EOF payload (dropped/truncated
            // frame) - discard it and resync on this payload instead.
            self.buf.clear();
        }
        self.last_fid = Some(fid);

        if self.buf.len() + data.len() <= self.max_frame_size {
            self.buf.extend_from_slice(data);
        }

        eof.then(|| std::mem::replace(&mut self.buf, Vec::with_capacity(self.max_frame_size)))
    }
}

/// Parses a UVC stream payload header, returning `(fid, eof, payload_data)`,
/// or `None` for an empty/malformed header or a payload marked as an error.
fn parse_payload_header(chunk: &[u8]) -> Option<(bool, bool, &[u8])> {
    let header_len = *chunk.first()? as usize;
    if header_len < 2 || header_len > chunk.len() {
        return None;
    }
    let flags = chunk[1];
    if flags & 0x40 != 0 {
        return None; // "Error Bit" set - drop this payload
    }
    let fid = flags & 0x01 != 0;
    let eof = flags & 0x02 != 0;
    Some((fid, eof, &chunk[header_len..]))
}

fn run_bulk(
    handle: &DeviceHandle<Context>,
    negotiated: &Negotiated,
    collector: &mut Collector,
) -> ah::Result<()> {
    let buf_size = (negotiated.max_payload_transfer_size as usize).max(16 * 1024);
    let mut buf = vec![0u8; buf_size];
    loop {
        let n = handle.read_bulk(negotiated.endpoint, &mut buf, BULK_TIMEOUT)?;
        collector.feed_payload(&buf[..n]);
    }
}

struct IsoUserData {
    collector: *mut Collector,
    stopping: bool,
    outstanding: usize,
    device_gone: bool,
}

/// Returns a pointer to the iso packet descriptor array that trails the
/// fixed fields of `transfer`.
///
/// This must use a raw place projection: `iso_packet_desc` is declared as a
/// zero-length array, so going through a reference (as
/// `iso_packet_desc.as_ptr()` would) yields a pointer whose provenance
/// covers zero bytes, and touching the descriptors through it would be UB.
///
/// # Safety
/// `transfer` must point at a valid `libusb_transfer`.
unsafe fn iso_packet_descs(
    transfer: *mut ffi::libusb_transfer,
) -> *mut ffi::libusb_iso_packet_descriptor {
    unsafe { (&raw mut (*transfer).iso_packet_desc).cast() }
}

fn run_iso(
    handle: &DeviceHandle<Context>,
    negotiated: &Negotiated,
    collector: &mut Collector,
) -> ah::Result<()> {
    let ctx_ptr = handle.context().as_raw();
    let packet_size = negotiated.packet_size.max(1);

    // The transfer callbacks access this through the raw `user_data` pointer
    // stored in each transfer, so `run_iso` must use the same raw pointer for
    // *every* access of its own: an access through a `Box` or `&mut` here
    // would invalidate the callbacks' pointer under Rust's aliasing rules.
    let user_data = Box::into_raw(Box::new(IsoUserData {
        collector: collector as *mut Collector,
        stopping: false,
        outstanding: 0,
        device_gone: false,
    }));

    // Buffers must stay at a stable address for as long as their transfer is
    // outstanding, so keep them alive here for the whole function.
    // `buffers` never reallocates (capacity is preallocated), so the content never moves.
    let mut buffers: Vec<Box<[u8]>> = Vec::with_capacity(ISO_TRANSFERS);
    let mut transfers: Vec<*mut ffi::libusb_transfer> = Vec::with_capacity(ISO_TRANSFERS);
    // How many of `transfers` were successfully handed to libusb.
    let mut submitted = 0;

    let run_result = (|| -> ah::Result<()> {
        for _ in 0..ISO_TRANSFERS {
            // SAFETY: `ISO_PACKETS_PER_TRANSFER` fits in a c_int, and the
            // returned transfer is checked for null before further use.
            let transfer = unsafe { ffi::libusb_alloc_transfer(ISO_PACKETS_PER_TRANSFER as i32) };
            if transfer.is_null() {
                return Err(err!("libusb_alloc_transfer() returned NULL"));
            }
            transfers.push(transfer);

            // Each one is moved into `buffers` *before* its data pointer is taken,
            // because moving a `Box` invalidates pointers derived from it.
            buffers.push(vec![0u8; packet_size * ISO_PACKETS_PER_TRANSFER].into_boxed_slice());
            let buffer = buffers.last_mut().expect("buffers cannot be empty");

            // SAFETY: `transfer` was just allocated with
            // `ISO_PACKETS_PER_TRANSFER` iso packet descriptors, `handle` is
            // a valid, open device handle, and `buffer` outlives the
            // transfer (kept in `buffers`, unmoved, until teardown below).
            unsafe {
                ffi::libusb_fill_iso_transfer(
                    transfer,
                    handle.as_raw(),
                    negotiated.endpoint,
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                    ISO_PACKETS_PER_TRANSFER as i32,
                    iso_callback,
                    user_data as *mut c_void,
                    0,
                );
                let descs = iso_packet_descs(transfer);
                for i in 0..ISO_PACKETS_PER_TRANSFER {
                    (*descs.add(i)).length = packet_size as u32;
                }
            }
        }

        for &transfer in &transfers {
            // SAFETY: `transfer` was just filled above and is not yet submitted.
            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                return Err(err!("libusb_submit_transfer() failed: {rc}"));
            }
            submitted += 1;
            // SAFETY: no callback can be running concurrently; callbacks
            // only fire from within `libusb_handle_events*` on this thread.
            unsafe { (*user_data).outstanding += 1 };
        }

        let timeout = libc::timeval {
            tv_sec: 1,
            tv_usec: 0,
        };
        loop {
            // SAFETY: `ctx_ptr` stays valid for as long as `handle` is alive.
            let rc = unsafe {
                ffi::libusb_handle_events_timeout_completed(ctx_ptr, &timeout, std::ptr::null_mut())
            };
            if rc != 0 {
                return Err(err!("libusb_handle_events() failed: {rc}"));
            }
            // SAFETY: only the callbacks (run from within the call above,
            // on this thread) ever write `device_gone`.
            if unsafe { (*user_data).device_gone } {
                return Err(err!("P2Pro USB device was disconnected"));
            }
        }
    })();

    // Teardown: ask every in-flight transfer to cancel, then keep pumping
    // the event loop until all of their completion callbacks (which free
    // them and decrement `outstanding`) have run.
    //
    // SAFETY (all `user_data` accesses below): callbacks only run from
    // within `libusb_handle_events_timeout_completed` on this thread, so
    // nothing accesses `user_data` concurrently with these accesses.
    unsafe { (*user_data).stopping = true };
    for &transfer in &transfers[..submitted] {
        // SAFETY: `transfer` is valid; cancelling a transfer that already
        // completed (and was not resubmitted) merely returns NOT_FOUND.
        unsafe {
            ffi::libusb_cancel_transfer(transfer);
        }
    }
    let timeout = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    // `outstanding` is decremented inside `handle_iso_completion` (invoked
    // from within `libusb_handle_events_timeout_completed`) as the cancelled
    // transfers complete, which clippy can't see through from here.
    #[allow(clippy::while_immutable_condition)]
    while unsafe { (*user_data).outstanding } > 0 {
        // SAFETY: `ctx_ptr` stays valid for as long as `handle` is alive.
        unsafe {
            ffi::libusb_handle_events_timeout_completed(ctx_ptr, &timeout, std::ptr::null_mut());
        }
    }
    // Transfers that were never handed to libusb are still ours to free. The
    // submitted ones were freed by their completion callback above (or
    // intentionally leaked on resubmit failure, see `handle_iso_completion`).
    for &transfer in &transfers[submitted..] {
        // SAFETY: `transfer` is valid and was never submitted.
        unsafe { ffi::libusb_free_transfer(transfer) };
    }
    // SAFETY: all callbacks have run; nothing references `user_data` anymore.
    drop(unsafe { Box::from_raw(user_data) });

    run_result
}

extern "system" fn iso_callback(transfer: *mut ffi::libusb_transfer) {
    // Rust panic must not unwind across C boundary.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `transfer` is a valid, just-completed transfer handed to
        // us by libusb, whose `user_data` was set by `run_iso` to a
        // `*mut IsoUserData` that outlives every in-flight transfer.
        unsafe { handle_iso_completion(transfer) }
    }));
    if result.is_err() {
        log::error!("P2Pro: panic in USB isochronous transfer callback");
    }
}

/// # Safety
/// `transfer` must be a valid, completed `libusb_transfer` whose `user_data`
/// points at a live `IsoUserData`.
unsafe fn handle_iso_completion(transfer: *mut ffi::libusb_transfer) {
    let user_data = unsafe { &mut *((*transfer).user_data as *mut IsoUserData) };
    let no_device = unsafe { (*transfer).status } == ffi::constants::LIBUSB_TRANSFER_NO_DEVICE;

    if !user_data.stopping && !no_device {
        let collector = user_data.collector;
        // Do not let panics propagate into our C caller.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: forwarded from the caller; `collector` points at the
            // `Collector` owned by `run`, which outlives the whole stream.
            unsafe { process_iso_packets(transfer, collector) }
        }));
        if result.is_err() {
            log::error!("P2Pro: panic while processing an iso transfer (frame dropped)");
        }
    }

    if user_data.stopping {
        user_data.outstanding -= 1;
        // SAFETY: the transfer completed and is no longer used by libusb.
        unsafe { ffi::libusb_free_transfer(transfer) };
        return;
    }

    // Do not resubmit once the device is confirmed gone or resubmission
    // fails (e.g. the device was unplugged) - stop tracking this transfer.
    // It leaks, but that is harmless: it only happens as the stream is
    // already on its way out. (It must not be freed here, because
    // `run_iso`'s teardown still cancels it.)
    if no_device || unsafe { ffi::libusb_submit_transfer(transfer) } != 0 {
        user_data.device_gone = true;
        user_data.outstanding -= 1;
    }
}

/// Feeds every successfully received packet of one completed isochronous
/// transfer into the collector.
///
/// # Safety
/// `transfer` must be a valid, completed isochronous `libusb_transfer` set
/// up by `run_iso` (equal-length packets, buffer of `num_iso_packets *
/// packet-length` bytes), and `collector` must point at a live `Collector`
/// that nothing else currently references.
unsafe fn process_iso_packets(transfer: *mut ffi::libusb_transfer, collector: *mut Collector) {
    let num_packets = unsafe { (*transfer).num_iso_packets } as usize;
    let descs = unsafe { iso_packet_descs(transfer) };
    let buffer = unsafe { (*transfer).buffer };
    if num_packets == 0 || buffer.is_null() {
        return;
    }
    // All packets were given the same length by `run_iso`.
    let packet_len = unsafe { (*descs).length } as usize;
    for i in 0..num_packets {
        // SAFETY: `i` is within the transfer's `num_iso_packets` descriptors.
        let desc = unsafe { &*descs.add(i) };
        if desc.status != ffi::constants::LIBUSB_TRANSFER_COMPLETED {
            continue;
        }
        let len = (desc.actual_length as usize).min(packet_len);
        if len == 0 {
            continue;
        }
        // SAFETY: packet `i` occupies the sub-slice
        // `buffer[i * packet_len ..][.. packet_len]` of the transfer buffer,
        // and libusb no longer writes to a completed transfer.
        let data = unsafe { std::slice::from_raw_parts(buffer.add(i * packet_len), len) };
        // SAFETY: the collector is only ever accessed from transfer
        // callbacks, which run sequentially on this one thread.
        unsafe { (*collector).feed_payload(data) };
    }
}
