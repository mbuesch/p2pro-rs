use crate::render::RenderedFrame;
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

fn encode_save_file_png(frame: &RenderedFrame) -> Vec<u8> {
    let mut png_bytes = Vec::with_capacity(1024 * 512);
    PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            &frame.rgba_bytes,
            frame.width,
            frame.height,
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
