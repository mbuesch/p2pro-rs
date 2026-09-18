use crate::render::RenderedFrame;
use crate::video::VideoTarget;
use chrono::prelude::*;
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};

fn make_filename() -> String {
    Local::now()
        .format("p2pro_%Y-%m-%d_%H-%M-%S.png")
        .to_string()
}

fn make_video_filename() -> String {
    Local::now()
        .format("p2pro_%Y-%m-%d_%H-%M-%S.avi")
        .to_string()
}

fn encode_save_file_png(frame: &RenderedFrame) -> Vec<u8> {
    let mut png_bytes = Vec::with_capacity(1024 * 512);
    PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            &frame.rgba.bytes,
            frame.meta.width,
            frame.meta.height,
            ExtendedColorType::Rgba8,
        )
        .expect("encoding a thermal frame to PNG should never fail");
    png_bytes
}

#[cfg(target_os = "linux")]
pub async fn save_frame_png(frame: &RenderedFrame) {
    if let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Save P2Pro thermal image")
        .set_file_name(make_filename())
        .add_filter("PNG image", &["png"])
        .save_file()
        .await
        && let Err(e) = tokio::fs::write(file.path(), &encode_save_file_png(frame)).await
    {
        eprintln!("Error: Saving the thermal image failed: {e}");
    }
}

#[cfg(target_os = "android")]
pub async fn save_frame_png(frame: &RenderedFrame) {
    use crate::camera::android::jni_bridge::save_file;
    if let Err(e) = save_file(&make_filename(), &encode_save_file_png(frame)).await {
        eprintln!("Error: Saving the thermal image failed: {e}");
    }
}

/// Opens the "save video as" file picker.
///
/// Returns the display name and the recording target, or `None` when the
/// user cancelled. The target is only materialized (created/truncated) once
/// the recording actually starts.
#[cfg(target_os = "linux")]
pub async fn pick_video_target() -> Option<VideoTarget> {
    let file = rfd::AsyncFileDialog::new()
        .set_title("Save P2Pro video")
        .set_file_name(make_video_filename())
        .add_filter("AVI video", &["avi"])
        .save_file()
        .await?;
    Some(VideoTarget::Path(file.path().to_path_buf()))
}

/// Opens the Android SAF "create document" picker for the video file.
/// The returned file is a seekable write descriptor (the AVI trailer
/// back-patches the header), owned by the recorder from here on.
#[cfg(target_os = "android")]
pub async fn pick_video_target() -> Option<VideoTarget> {
    use crate::camera::android::jni_bridge::pick_video_file;
    use std::os::fd::FromRawFd;
    let fd = pick_video_file(&make_video_filename()).await.ok()??;
    // SAFETY: the activity handed us a freshly opened, detached fd.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    Some(VideoTarget::File(file))
}
