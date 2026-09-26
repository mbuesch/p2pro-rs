//! Reads UVC payload data off the negotiated endpoint (bulk or isochronous)
//! and reassembles it into complete thermal frames.

use crate::{
    app::FromUi,
    camera::{CaptureState, HEIGHT, WIDTH, android::protocol::Negotiated, decode_frame},
    render::Renderer,
};
use anyhow::{self as ah, Context as _, format_err as err};
use rusb::{Context, DeviceHandle, TransferType, UsbContext, constants::LIBUSB_ERROR_TIMEOUT, ffi};
use std::{
    ptr::null_mut,
    slice,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};

/// Full raw YUYV frame size: video half on top, thermal half on the bottom.
const FRAME_BYTES: usize = WIDTH as usize * 2 * (HEIGHT as usize * 2);

const FRAME_TIMEOUT: Duration = Duration::from_millis(200);
const ISO_TRANSFERS: usize = 4;
const ISO_PACKETS_PER_TRANSFER: usize = 32;

fn duration_to_timeval(duration: Duration) -> libc::timeval {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: duration.as_micros().try_into().expect("tv_usec"),
    };
    while tv.tv_usec >= 1_000_000 {
        tv.tv_sec += 1;
        tv.tv_usec -= 1_000_000;
    }
    tv
}

pub async fn run(
    handle: Arc<DeviceHandle<Context>>,
    negotiated: Negotiated,
    to_ui: mpsc::Sender<CaptureState>,
    from_ui: watch::Receiver<FromUi>,
) -> ah::Result<()> {
    let collector = Collector {
        reassembler: FrameReassembler::new(FRAME_BYTES),
        renderer: Renderer::new(),
        to_ui,
        from_ui,
    };

    match negotiated.transfer_type {
        TransferType::Bulk => run_bulk(handle, negotiated, collector).await,
        TransferType::Isochronous => run_iso(handle, negotiated, collector).await,
        t => Err(err!("Unsupported UVC video endpoint transfer type: {t:?}")),
    }
}

/// Accumulates UVC payloads into frames and forwards decoded frames to the UI.
struct Collector {
    reassembler: FrameReassembler,
    renderer: Renderer,
    to_ui: mpsc::Sender<CaptureState>,
    from_ui: watch::Receiver<FromUi>,
}

impl Collector {
    fn feed_payload(&mut self, chunk: &[u8]) {
        if let Some(frame_bytes) = self.reassembler.feed(chunk) {
            let from_ui = self.from_ui.borrow().clone();
            if let Some(frame) = decode_frame(
                &mut self.renderer,
                &frame_bytes,
                WIDTH as usize * 2,
                &from_ui,
            ) {
                let _ = self.to_ui.blocking_send(CaptureState::Frame(frame));
            }
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
    let header_len: usize = (*chunk.first()?).into();
    if header_len < 2 || header_len > chunk.len() {
        None
    } else {
        let flags = chunk[1];
        if flags & 0x40 == 0 {
            let fid = flags & 0x01 != 0;
            let eof = flags & 0x02 != 0;
            Some((fid, eof, &chunk[header_len..]))
        } else {
            None // "Error Bit" set - drop this payload
        }
    }
}

async fn run_bulk(
    handle: Arc<DeviceHandle<Context>>,
    negotiated: Negotiated,
    mut collector: Collector,
) -> ah::Result<()> {
    let buf_size: usize = negotiated
        .max_payload_transfer_size
        .try_into()
        .context("Buffer size")?;
    let buf_size = buf_size.max(16 * 1024);

    loop {
        let handle = Arc::clone(&handle);

        let res = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0_u8; buf_size];
            let n = match handle.read_bulk(negotiated.endpoint, &mut buf, FRAME_TIMEOUT) {
                Ok(n) => n,
                Err(e) => return Err((e, collector)),
            };
            collector.feed_payload(&buf[..n]);
            Ok(collector)
        })
        .await
        .context("Tokio task failed")?;

        match res {
            Ok(c) => collector = c,
            Err((rusb::Error::Timeout, c)) => collector = c,
            Err((e, _c)) => return Err(e).context("Bulk transfer failed")?,
        }
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

async fn run_iso(
    handle: Arc<DeviceHandle<Context>>,
    negotiated: Negotiated,
    mut collector: Collector,
) -> ah::Result<()> {
    let packet_size = negotiated.packet_size.max(1);

    // The transfer callbacks access this through the raw `user_data` pointer
    // stored in each transfer, so `run_iso` must use the same raw pointer for
    // *every* access of its own: an access through a `Box` or `&mut` here
    // would invalidate the callbacks' pointer under Rust's aliasing rules.
    // AtomicPtr makes the pointer carrier Send + Sync for the async future;
    // callback access to the pointed-to state is still serialized below.
    let user_data = Arc::new(AtomicPtr::new(Box::into_raw(Box::new(IsoUserData {
        collector: &raw mut collector,
        stopping: false,
        outstanding: 0,
        device_gone: false,
    }))));

    // Buffers must stay at a stable address for as long as their transfer is
    // outstanding, so keep them alive here for the whole function.
    // `buffers` never reallocates (capacity is preallocated), so the content never moves.
    let mut buffers: Vec<Box<[u8]>> = Vec::with_capacity(ISO_TRANSFERS);
    let transfers: Arc<StdMutex<Vec<AtomicPtr<ffi::libusb_transfer>>>> =
        Arc::new(StdMutex::new(Vec::with_capacity(ISO_TRANSFERS)));
    // How many of `transfers` were successfully handed to libusb.
    let submitted = Arc::new(AtomicUsize::new(0));

    let run_result = {
        let handle = Arc::clone(&handle);
        let user_data = Arc::clone(&user_data);
        let transfers = Arc::clone(&transfers);
        let submitted = Arc::clone(&submitted);

        (async move || -> ah::Result<()> {
            for _ in 0..ISO_TRANSFERS {
                // SAFETY: The allocated size is sufficient.
                let transfer = unsafe {
                    ffi::libusb_alloc_transfer(
                        ISO_PACKETS_PER_TRANSFER
                            .try_into()
                            .context("ISO_PACKETS_PER_TRANSFER")?,
                    )
                };
                if transfer.is_null() {
                    return Err(err!("libusb_alloc_transfer() returned NULL"));
                }
                transfers
                    .lock()
                    .expect("Lock poisoned")
                    .push(AtomicPtr::new(transfer));

                // Each one is moved into `buffers` *before* its data pointer is taken,
                // because moving a `Box` invalidates pointers derived from it.
                buffers.push(vec![0_u8; packet_size * ISO_PACKETS_PER_TRANSFER].into_boxed_slice());
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
                        buffer.len().try_into().context("Buffer length")?,
                        ISO_PACKETS_PER_TRANSFER
                            .try_into()
                            .context("ISO_PACKETS_PER_TRANSFER")?,
                        iso_callback,
                        user_data.load(Ordering::Relaxed).cast(),
                        0,
                    );
                    let descs = iso_packet_descs(transfer);
                    for i in 0..ISO_PACKETS_PER_TRANSFER {
                        (*descs.add(i)).length = packet_size.try_into().context("Packet size")?;
                    }
                }
            }

            for transfer in transfers.lock().expect("Lock poisoned").iter() {
                // SAFETY: `transfer` was just filled above and is not yet submitted.
                let rc = unsafe { ffi::libusb_submit_transfer(transfer.load(Ordering::Relaxed)) };
                if rc != 0 {
                    return Err(err!("libusb_submit_transfer() failed: {rc}"));
                }
                submitted.fetch_add(1, Ordering::Relaxed);
                // SAFETY: no callback can be running concurrently; callbacks
                // only fire from within `libusb_handle_events*` on this thread.
                let user_data = user_data.load(Ordering::Relaxed);
                unsafe { (*user_data).outstanding += 1 };
            }

            loop {
                let handle = Arc::clone(&handle);

                let rc = tokio::task::spawn_blocking(move || {
                    // SAFETY: `handle` is alive.
                    let rc = unsafe {
                        ffi::libusb_handle_events_timeout_completed(
                            handle.context().as_raw(),
                            &duration_to_timeval(FRAME_TIMEOUT),
                            null_mut(),
                        )
                    };
                    rc
                })
                .await
                .context("Tokio task failed")?;

                if rc != 0 && rc != LIBUSB_ERROR_TIMEOUT {
                    return Err(err!("libusb_handle_events() failed: {rc}"));
                }
                // SAFETY: only the callbacks (run from within the call above,
                // on this thread) ever write `device_gone`.
                let user_data = user_data.load(Ordering::Relaxed);
                if unsafe { (*user_data).device_gone } {
                    return Err(err!("P2Pro USB device was disconnected"));
                }
            }
        })()
        .await
    };

    // Shutdown and clean up.

    let user_data = user_data.load(Ordering::Relaxed);
    let submitted = submitted.load(Ordering::Relaxed);

    // Teardown: ask every in-flight transfer to cancel, then keep pumping
    // the event loop until all of their completion callbacks (which free
    // them and decrement `outstanding`) have run.
    //
    // SAFETY (all `user_data` accesses below): callbacks only run from
    // within `libusb_handle_events_timeout_completed` on this thread, so
    // nothing accesses `user_data` concurrently with these accesses.
    unsafe { (*user_data).stopping = true };
    for transfer in &transfers.lock().expect("Lock poisoned")[..submitted] {
        // SAFETY: `transfer` is valid; cancelling a transfer that already
        // completed (and was not resubmitted) merely returns NOT_FOUND.
        unsafe {
            ffi::libusb_cancel_transfer(transfer.load(Ordering::Relaxed));
        }
    }

    // `outstanding` is decremented inside `handle_iso_completion` (invoked
    // from within `libusb_handle_events_timeout_completed`) as the cancelled
    // transfers complete, which clippy can't see through from here.
    #[allow(clippy::while_immutable_condition)]
    while unsafe { (*user_data).outstanding } > 0 {
        // SAFETY: `handle` is alive.
        unsafe {
            ffi::libusb_handle_events_timeout_completed(
                handle.context().as_raw(),
                &duration_to_timeval(Duration::from_secs(1)),
                std::ptr::null_mut(),
            );
        }
    }

    // Transfers that were never handed to libusb are still ours to free. The
    // submitted ones were freed by their completion callback above (or
    // intentionally leaked on resubmit failure, see `handle_iso_completion`).
    for transfer in &transfers.lock().expect("Lock poisoned")[submitted..] {
        // SAFETY: `transfer` is valid and was never submitted.
        unsafe { ffi::libusb_free_transfer(transfer.load(Ordering::Relaxed)) };
    }

    // SAFETY: all callbacks have run; nothing references `user_data` anymore.
    drop(unsafe { Box::from_raw(user_data) });

    run_result
}

extern "system" fn iso_callback(transfer: *mut ffi::libusb_transfer) {
    if transfer.is_null() {
        return;
    }
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
    if transfer.is_null() {
        return;
    }
    // SAFETY: `transfer` is a valid, completed isochronous `libusb_transfer`.
    let (user_data, no_device) = unsafe {
        (
            &mut *((*transfer).user_data as *mut IsoUserData),
            (*transfer).status == ffi::constants::LIBUSB_TRANSFER_NO_DEVICE,
        )
    };

    if !user_data.stopping && !no_device {
        // SAFETY: forwarded from the caller; `collector` points at the
        // `Collector` owned by `run`, which outlives the whole stream.
        unsafe { process_iso_packets(transfer, user_data.collector) }
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
    // SAFETY: `transfer` is a valid.
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
    if transfer.is_null() || collector.is_null() {
        return;
    }
    // SAFETY: `transfer` is a valid, completed isochronous `libusb_transfer`.
    let (num_packets, descs, buffer) = unsafe {
        (
            (*transfer)
                .num_iso_packets
                .try_into()
                .expect("num_iso_packets"),
            iso_packet_descs(transfer),
            (*transfer).buffer,
        )
    };
    if descs.is_null() || buffer.is_null() {
        return;
    }
    for i in 0..num_packets {
        // SAFETY: `i` is within the transfer's `num_iso_packets` descriptors.
        let desc = unsafe { &*descs.add(i) };
        let packet_len = desc.length.try_into().expect("packet length");
        if desc.status != ffi::constants::LIBUSB_TRANSFER_COMPLETED {
            continue;
        }
        let len: usize = desc.actual_length.try_into().expect("actual length");
        let len = len.min(packet_len);
        if len > 0 {
            // SAFETY: packet `i` occupies the sub-slice
            // `buffer[i * packet_len ..][.. packet_len]` of the transfer buffer,
            // and libusb no longer writes to a completed transfer.
            let data = unsafe { slice::from_raw_parts(buffer.add(i * packet_len), len) };
            // SAFETY: the collector is only ever accessed from transfer
            // callbacks, which run sequentially on this one thread.
            unsafe { (*collector).feed_payload(data) };
        }
    }
}
