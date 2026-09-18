//! Lossless video recording of the rendered frames into a HuffYUV AVI file.

use crate::render::{RenderedFrame, RenderedFrameRgba};
use anyhow::{self as ah, Context as _, format_err as err};
use oxideav_avi::muxer;
use oxideav_core::{
    CodecId, CodecParameters, CodecTag, MediaType, Muxer, Packet, Rational, StreamInfo, TimeBase,
    WriteSeek,
};
use oxideav_huffyuv::{ExtradataMode, Method, PixelFamily, encode_frame_with_mode};
use std::{
    fs::File,
    path::PathBuf,
    slice,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::{self, JoinHandle},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc as tokio_mpsc};

/// Video stream time base: 25 fps (the camera's native frame rate).
const TIME_BASE: TimeBase = TimeBase::new(1, 25);

#[derive(Debug)]
/// A destination for the recorded video file.
pub enum VideoTarget {
    /// Linux desktop: a file path, created lazily when recording starts.
    Path(PathBuf),
    /// Android: an already-open, seekable file (SAF descriptor handed over
    /// by the activity).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    File(File),
}

#[derive(Debug)]
/// Commands sent from the UI to the writer thread.
enum VideoCmd {
    /// Begin recording into the given output file.
    Start(VideoTarget),
    /// One rendered frame to encode and append.
    Frame {
        width: u32,
        height: u32,
        rgba: Arc<RenderedFrameRgba>,
    },
    /// Finish the recording: write the trailer and close the file.
    Stop,
    /// Like `Stop`, but also exit the writer thread.
    Shutdown,
}

/// Events reported from the writer thread to the UI.
#[derive(Clone, Debug)]
pub enum VideoEvent {
    /// The recording was finalized (trailer written).
    /// Carries the number of frames written.
    Stopped { frames: u64 },
    /// A recording operation failed; the recorder is back to idle.
    Error(String),
}

/// An open recording session on the writer thread.
struct OpenRec {
    mux: Box<dyn Muxer>,
    frames: u64,
    width: u32,
    height: u32,
}

/// Converts a top-down RGBA8 raster into the HuffYUV RGB32 wire layout
/// (packed BGRA, bottom-up row order).
pub fn rgba_to_bgra_bottom_up(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0_u8; w * h * 4];
    if rgba.len() != out.len() {
        return out;
    }
    for y in 0..h {
        let src_row = y * w * 4;
        let dst_row = (h - 1 - y) * w * 4;
        for x in 0..w {
            let s = src_row + x * 4;
            let d = dst_row + x * 4;
            out[d] = rgba[s + 2]; // B
            out[d + 1] = rgba[s + 1]; // G
            out[d + 2] = rgba[s]; // R
            out[d + 3] = rgba[s + 3]; // A
        }
    }
    out
}

/// A synthetic 256x192 top-down RGBA frame with a diagonal gradient.
pub fn synth_rgba(frame_num: u8) -> Vec<u8> {
    let (w, h) = (256_usize, 192_usize);
    let mut v = vec![0_u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let off = (y * w + x) * 4;
            v[off] = ((x + y + frame_num as usize) & 0xFF) as u8;
            v[off + 1] = ((x * 3 + y * 5) & 0xFF) as u8;
            v[off + 2] = ((x * 7 + y * 11) & 0xFF) as u8;
            v[off + 3] = 0xFF;
        }
    }
    v
}

/// Encodes one frame as HuffYUV (RGB32 family, classic v2.x extradata).
///
/// The method is `Left` (byte 0x00), *not* the wire-equivalent
/// `predict_old` (0xFE): FFmpeg's HuffYUV decoder (and with it VLC and
/// mpv) parses the v2 extradata method byte as `method & 63`, so 0xFE
/// yields an unsupported predictor and every frame is dropped.
/// `predict_old` and `Left` produce the identical bitstream,
/// so this stays lossless and third-party-decodable.
///
/// Returns `(strf, frame_bytes)`; the `strf` is
/// identical for all frames of a stream and only needed to open the muxer.
fn encode_huffyuv(width: u32, height: u32, rgba: &[u8]) -> ah::Result<(Vec<u8>, Vec<u8>)> {
    let len = rgba.len();
    if len != (width as usize) * (height as usize) * 4 {
        return Err(err!("Frame buf size mismatch: {len} for {width}x{height}"));
    }
    let wire = rgba_to_bgra_bottom_up(width, height, rgba);
    encode_frame_with_mode(
        PixelFamily::Rgb32,
        Method::Left,
        width,
        height,
        &wire,
        ExtradataMode::ClassicV2,
    )
    .map_err(|e| err!("HuffYUV encoding failed: {e}"))
}

/// Opens the AVI muxer for a stream of the given dimensions and writes the
/// header. The HuffYUV extradata is the encoder's `strf` minus its 40-byte
/// BITMAPINFOHEADER prefix (the muxer rewrites the BIH itself).
fn open_recording(file: File, width: u32, height: u32, rgba: &[u8]) -> ah::Result<OpenRec> {
    let (strf, _) = encode_huffyuv(width, height, rgba)?;
    let extradata = if strf.len() > 40 {
        strf[40..].to_vec()
    } else {
        vec![]
    };

    let mut params = CodecParameters::video(CodecId::new("huffyuv"));
    params.media_type = MediaType::Video;
    params.tag = Some(CodecTag::fourcc(b"HFYU"));
    params.width = Some(width);
    params.height = Some(height);
    params.frame_rate = Some(Rational::new(25, 1));
    params.extradata = extradata;
    let stream = StreamInfo {
        index: 0,
        time_base: TIME_BASE,
        duration: None,
        start_time: Some(0),
        params,
    };

    let output: Box<dyn WriteSeek> = Box::new(file);
    let stream = slice::from_ref(&stream);
    let mut mux = muxer::open(output, stream).context("Opening AVI muxer failed")?;
    mux.write_header().context("Writing AVI header failed")?;

    Ok(OpenRec {
        mux,
        frames: 0,
        width,
        height,
    })
}

/// Encodes and appends one frame to an open recording.
fn write_video_frame(rec: &mut OpenRec, rgba: &[u8]) -> ah::Result<()> {
    let (_, encoded) = encode_huffyuv(rec.width, rec.height, rgba)?;
    let mut pkt = Packet::new(0, TIME_BASE, encoded);
    pkt.pts = Some(rec.frames as i64);
    pkt.flags.keyframe = true;
    rec.mux
        .write_packet(&pkt)
        .context("Writing a video frame failed")?;
    rec.frames += 1;
    Ok(())
}

/// Finalize an open recording.
fn finalize(open: &mut Option<OpenRec>, events: &tokio_mpsc::UnboundedSender<VideoEvent>) {
    if let Some(mut rec) = open.take() {
        let event = match rec.mux.write_trailer() {
            Ok(()) => VideoEvent::Stopped { frames: rec.frames },
            Err(e) => VideoEvent::Error(format!("Finalizing the video file failed: {e:?}")),
        };
        let _ = events.send(event);
    } else {
        let _ = events.send(VideoEvent::Stopped { frames: 0 });
    }
}

/// Drops an open recording without finalizing
fn abort(
    open: &mut Option<OpenRec>,
    events: &tokio_mpsc::UnboundedSender<VideoEvent>,
    msg: String,
) {
    open.take();
    let _ = events.send(VideoEvent::Error(msg));
}

fn writer_main(rx: Receiver<VideoCmd>, events: tokio_mpsc::UnboundedSender<VideoEvent>) {
    // The opened output file, waiting for the first frame (the muxer needs
    // the frame dimensions to build the stream header).
    let mut pending: Option<File> = None;
    // The open recording session.
    let mut open: Option<OpenRec> = None;

    for cmd in rx {
        match cmd {
            VideoCmd::Start(target) => {
                if open.is_some() || pending.is_some() {
                    continue;
                }
                match target {
                    VideoTarget::File(file) => pending = Some(file),
                    VideoTarget::Path(path) => match File::create(&path) {
                        Ok(file) => pending = Some(file),
                        Err(e) => {
                            let _ = events.send(VideoEvent::Error(format!(
                                "Cannot create video file {}: {e}",
                                path.display()
                            )));
                        }
                    },
                }
            }
            VideoCmd::Frame {
                width,
                height,
                rgba,
            } => {
                if let Some(rec) = &mut open {
                    if rec.width != width || rec.height != height {
                        eprintln!(
                            "Video: frame size changed from {}x{} to {width}x{height}; skipping frame",
                            rec.width, rec.height
                        );
                        continue;
                    }
                    if let Err(e) = write_video_frame(rec, &rgba.bytes) {
                        abort(&mut open, &events, format!("{e:?}"));
                    }
                } else if let Some(file) = pending.take() {
                    match open_recording(file, width, height, &rgba.bytes) {
                        Ok(mut rec) => {
                            if let Err(e) = write_video_frame(&mut rec, &rgba.bytes) {
                                let _ = events.send(VideoEvent::Error(format!("{e:?}")));
                            } else {
                                open = Some(rec);
                            }
                        }
                        Err(e) => {
                            let _ = events.send(VideoEvent::Error(format!("{e:?}")));
                        }
                    }
                }
            }
            VideoCmd::Stop => {
                pending = None;
                finalize(&mut open, &events);
            }
            VideoCmd::Shutdown => {
                // A pending (never-started) file has no header yet; just drop it.
                finalize(&mut open, &events);
                break;
            }
        }
    }
}

/// Handle to the video writer thread.
#[derive(Clone, Debug)]
pub struct VideoRecorder {
    tx: Sender<VideoCmd>,
    recording: Arc<AtomicBool>,
    events: Arc<AsyncMutex<tokio_mpsc::UnboundedReceiver<VideoEvent>>>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl PartialEq for VideoRecorder {
    /// Identity by shared state (all clones of one recorder are equal).
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.recording, &other.recording)
    }
}

impl VideoRecorder {
    /// Spawns the writer thread.
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let (event_tx, event_rx) = tokio_mpsc::unbounded_channel();
        let worker = thread::spawn(move || writer_main(rx, event_tx));
        Self {
            tx,
            recording: Arc::new(AtomicBool::new(false)),
            events: Arc::new(AsyncMutex::new(event_rx)),
            worker: Arc::new(Mutex::new(Some(worker))),
        }
    }

    /// Starts recording into the given output file.
    /// Frames passed to [`Self::push_frame`] from now on are written.
    pub fn start(&self, target: VideoTarget) {
        self.recording.store(true, Ordering::Relaxed);
        let _ = self.tx.send(VideoCmd::Start(target));
    }

    /// Whether frames are currently being forwarded to the writer.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Forwards one rendered RGBA frame to the recorder.
    pub fn push_frame(&self, frame: &RenderedFrame) {
        if self.is_recording() {
            let _ = self.tx.send(VideoCmd::Frame {
                width: frame.meta.width,
                height: frame.meta.height,
                rgba: Arc::clone(&frame.rgba),
            });
        }
    }

    /// Finishes the recording (writes the trailer).
    pub fn stop(&self) {
        self.recording.store(false, Ordering::Relaxed);
        let _ = self.tx.send(VideoCmd::Stop);
    }

    /// Receives the next writer event. Awaits until one is available.
    pub async fn next_event(&self) -> Option<VideoEvent> {
        self.events.lock().await.recv().await
    }

    /// Finalizes any ongoing recording and joins the writer thread.
    pub fn shutdown(&self) {
        self.recording.store(false, Ordering::Relaxed);
        let _ = self.tx.send(VideoCmd::Shutdown);
        let handle = self.worker.lock().expect("Lock poisoned").take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Default for VideoRecorder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{NullCodecResolver, ReadSeek};
    use oxideav_huffyuv::{StreamConfig, decode_frame};
    use std::{env::temp_dir, fs, io, process};

    #[test]
    fn bgra_conversion_layout() {
        // 2x2 pixels, distinct channel values, top-down:
        // (r0: A,B) (r1: C,D) -> bottom-up rows: (C,D) then (A,B), BGRA each.
        #[rustfmt::skip]
        let rgba = vec![
            1,  2,  3,  4,    5,  6,  7,  8,
            9, 10, 11, 12,   13, 14, 15, 16,
        ];
        #[rustfmt::skip]
        let expect = vec![
            11, 10, 9, 12,   15, 14, 13, 16,
            3,   2, 1,  4,    7,  6,  5,  8,
        ];
        assert_eq!(rgba_to_bgra_bottom_up(2, 2, &rgba), expect);
    }

    /// Full production-path round-trip:
    /// open_recording -> write_video_frame -> write_trailer,
    /// then demux + decode and compare pixels.
    #[test]
    fn record_roundtrip() {
        let (width, height) = (256_u32, 192_u32);
        let frames: Vec<Vec<u8>> = (0..10).map(synth_rgba).collect();

        let tmp = temp_dir().join(format!("p2pro-rs-video-{}.avi", process::id()));
        let file = fs::File::create(&tmp).expect("create temp file failed");
        let mut rec =
            open_recording(file, width, height, &frames[0]).expect("open_recording failed");
        for rgba in &frames {
            write_video_frame(&mut rec, rgba).expect("write_video_frame failed");
        }
        assert_eq!(rec.frames, frames.len() as u64);
        rec.mux.write_trailer().expect("write_trailer failed");
        drop(rec);

        let avi_bytes = fs::read(&tmp).expect("read temp file failed");
        let _ = fs::remove_file(&tmp);
        assert!(!avi_bytes.is_empty());

        // Demux and decode everything back.
        let cursor: Box<dyn ReadSeek> = Box::new(io::Cursor::new(avi_bytes));
        let mut dmx =
            oxideav_avi::demuxer::open(cursor, &NullCodecResolver).expect("AVI demux open failed");
        assert_eq!(dmx.streams().len(), 1);
        assert_eq!(dmx.streams()[0].params.tag, Some(CodecTag::fourcc(b"HFYU")));
        assert_eq!(dmx.streams()[0].params.width, Some(width));
        assert_eq!(dmx.streams()[0].params.height, Some(height));

        // Rebuild the StreamConfig from the production encoder's strf.
        let (strf, _) = encode_huffyuv(width, height, &frames[0]).expect("encode failed");
        // FFmpeg compatibility lock: the v2 extradata method byte (first
        // byte after the 40-byte BIH) must be 0x00 (Left). 0xFE
        // (predict_old) is rejected by FFmpeg's HuffYUV decoder.
        assert_eq!(strf[40], 0x00, "extradata method byte must be Left");
        let cfg = StreamConfig::parse_bitmapinfoheader(&strf).expect("strf parse failed");

        for (i, rgba) in frames.iter().enumerate() {
            let pkt = dmx.next_packet().expect("packet missing");
            assert!(pkt.flags.keyframe);
            assert_eq!(pkt.pts, Some(i as i64));
            let decoded = decode_frame(&cfg, &pkt.data).expect("huffyuv decode failed");
            assert_eq!(decoded.width, width);
            assert_eq!(decoded.height, height);
            assert_eq!(
                decoded.pixels,
                rgba_to_bgra_bottom_up(width, height, rgba),
                "decoded frame {i} differs from the original raster"
            );
        }
    }
}
