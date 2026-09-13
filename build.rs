use std::{env, fs, path::PathBuf};

fn build_icon() {
    let png_path = "assets/icon-128x128.png";
    println!("cargo:rerun-if-changed={png_path}");

    let bytes = fs::read(png_path).expect("Failed to read window icon PNG");
    let image = image::load_from_memory(&bytes).expect("Failed to decode window icon PNG");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut out = Vec::with_capacity(4 + 4 + rgba.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(rgba.as_raw());

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set")).join("icon.rgba");
    fs::write(&out_path, out).expect("Failed to write prebuilt window icon");
}

fn main() {
    build_icon();
}
