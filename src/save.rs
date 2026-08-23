use crate::camera::ThermalFrame;
use chrono::prelude::*;

fn make_filename() -> String {
    Local::now()
        .format("p2pro_%Y-%m-%d_%H-%M-%S.png")
        .to_string()
}

#[cfg(target_os = "linux")]
pub async fn save_frame_png(frame: &ThermalFrame) {
    if let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Save P2Pro thermal image")
        .set_file_name(make_filename())
        .add_filter("PNG image", &["png"])
        .save_file()
        .await
        && let Err(e) = tokio::fs::write(file.path(), &frame.png_bytes).await
    {
        eprintln!("Error: Saving the thermal image failed: {e}");
    }
}

#[cfg(target_os = "android")]
pub async fn save_frame_png(frame: &ThermalFrame) {
    //TODO
}
