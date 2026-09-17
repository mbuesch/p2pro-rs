//! Round-trip test for the video pipeline:
//! HuffYUV encode -> AVI mux -> AVI demux -> HuffYUV decode,
//! asserting pixel-exact equality.
//!
//! This mirrors the codec/container flow used by `src/video.rs` and proves
//! that the published oxideav-avi muxer accepts a HuffYUV stream carrying an
//! explicit `HFYU` FourCC tag.

use oxideav_core::{
    CodecId, CodecParameters, CodecTag, MediaType, NullCodecResolver, Packet, Rational, ReadSeek,
    StreamInfo, TimeBase, WriteSeek,
};
use oxideav_huffyuv::{
    ExtradataMode, Method, PixelFamily, StreamConfig, decode_frame, encode_frame_with_mode,
};
use p2pro_rs::video::{rgba_to_bgra_bottom_up, synth_rgba};
use std::{env::temp_dir, fs, process, slice};

/// Strips the 40-byte BITMAPINFOHEADER prefix off the encoder's `strf`,
/// leaving the per-codec extradata the AVI muxer writes after its own BIH.
fn strip_bih(strf: &[u8]) -> Vec<u8> {
    if strf.len() <= 40 {
        vec![]
    } else {
        strf[40..].to_vec()
    }
}

#[test]
fn huffyuv_avi_roundtrip() {
    let (width, height) = (256_u32, 192_u32);
    let n_frames = 3_u8;

    // Build the AVI stream description once; all frames share the strf.
    // Method::Left (not the wire-equivalent PredictOld): FFmpeg's HuffYUV
    // decoder rejects the 0xFE method byte that PredictOld writes into the
    // v2 extradata.
    let frames: Vec<Vec<u8>> = (0..n_frames).map(synth_rgba).collect();
    let first_wire = rgba_to_bgra_bottom_up(width, height, &frames[0]);
    let (strf, _) = encode_frame_with_mode(
        PixelFamily::Rgb32,
        Method::Left,
        width,
        height,
        &first_wire,
        ExtradataMode::ClassicV2,
    )
    .expect("huffyuv encode failed");
    assert_eq!(strf[40], 0x00, "extradata method byte must be Left");

    let mut params = CodecParameters::video(CodecId::new("huffyuv"));
    params.media_type = MediaType::Video;
    params.tag = Some(CodecTag::fourcc(b"HFYU"));
    params.width = Some(width);
    params.height = Some(height);
    params.frame_rate = Some(Rational::new(25, 1));
    params.extradata = strip_bih(&strf);
    let stream = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, 25),
        duration: None,
        start_time: Some(0),
        params,
    };

    // Encode + mux all frames into a temporary AVI file.
    let tmp = temp_dir().join(format!("p2pro-rs-roundtrip-{}.avi", process::id()));
    {
        let f = fs::File::create(&tmp).expect("create temp file failed");
        let ws: Box<dyn WriteSeek> = Box::new(f);
        let mut mux = oxideav_avi::muxer::open(ws, slice::from_ref(&stream)).unwrap_or_else(|e| {
            panic!("AVI muxer rejected the HuffYUV stream config: {e}");
        });
        mux.write_header().expect("write_header failed");

        for (i, rgba) in frames.iter().enumerate() {
            let wire = rgba_to_bgra_bottom_up(width, height, rgba);
            let (_, encoded) = encode_frame_with_mode(
                PixelFamily::Rgb32,
                Method::Left,
                width,
                height,
                &wire,
                ExtradataMode::ClassicV2,
            )
            .expect("huffyuv encode failed");
            let mut pkt = Packet::new(0, stream.time_base, encoded);
            pkt.pts = Some(i as i64);
            pkt.flags.keyframe = true;
            mux.write_packet(&pkt).expect("write_packet failed");
        }
        mux.write_trailer().expect("write_trailer failed");
    }
    let avi_bytes = fs::read(&tmp).expect("read temp file failed");
    let _ = fs::remove_file(&tmp);
    assert!(!avi_bytes.is_empty(), "muxer produced an empty file");

    // Demux and decode everything back.
    let cursor: Box<dyn ReadSeek> = Box::new(std::io::Cursor::new(avi_bytes));
    let mut dmx =
        oxideav_avi::demuxer::open(cursor, &NullCodecResolver).expect("AVI demux open failed");
    assert_eq!(dmx.streams().len(), 1);
    assert_eq!(dmx.streams()[0].params.tag, Some(CodecTag::fourcc(b"HFYU")));
    assert_eq!(dmx.streams()[0].params.width, Some(width));
    assert_eq!(dmx.streams()[0].params.height, Some(height));

    let cfg = StreamConfig::parse_bitmapinfoheader(&strf).expect("strf parse failed");
    for (i, rgba) in frames.iter().enumerate() {
        let pkt = dmx.next_packet().expect("packet missing");
        assert!(pkt.flags.keyframe);
        let decoded = decode_frame(&cfg, &pkt.data).expect("huffyuv decode failed");
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
        let expected_wire = rgba_to_bgra_bottom_up(width, height, rgba);
        assert_eq!(
            decoded.pixels, expected_wire,
            "decoded frame {i} differs from the original raster"
        );
    }
}
