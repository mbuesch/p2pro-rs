//! Turns a temperature grid into a false-color PNG (as a data: URI) plus the
//! min/max statistics needed to draw markers and the legend.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

/// Result of rendering one frame: a ready-to-embed PNG data URI, the
/// min/max temperatures found, and their pixel coordinates.
pub struct RenderedFrame {
    pub data_uri: String,
    pub min_temp: f32,
    pub max_temp: f32,
    pub min_pos: (u32, u32),
    pub max_pos: (u32, u32),
}

/// Maps `temps` (row-major, `width` x `height`) through `lut` after
/// auto-scaling to the frame's own min/max, and encodes the result as PNG.
pub fn build_frame(width: u32, height: u32, temps: &[f32], lut: &[[u8; 4]]) -> RenderedFrame {
    let mut min_temp = f32::MAX;
    let mut max_temp = f32::MIN;
    let mut min_pos = (0u32, 0u32);
    let mut max_pos = (0u32, 0u32);

    for (i, &t) in temps.iter().enumerate() {
        let x = i as u32 % width;
        let y = i as u32 / width;
        if t < min_temp {
            min_temp = t;
            min_pos = (x, y);
        }
        if t > max_temp {
            max_temp = t;
            max_pos = (x, y);
        }
    }

    // Auto-scale: the color range always spans exactly this frame's min/max.
    let range = (max_temp - min_temp).max(0.1);

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (i, &t) in temps.iter().enumerate() {
        let n = (((t - min_temp) / range) * 255.0).clamp(0.0, 255.0) as usize;
        rgba[i * 4..i * 4 + 4].copy_from_slice(&lut[n]);
    }

    let mut png_bytes = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        .expect("encoding a thermal frame to PNG should never fail");
    let data_uri = format!("data:image/png;base64,{}", STANDARD.encode(&png_bytes));

    RenderedFrame {
        data_uri,
        min_temp,
        max_temp,
        min_pos,
        max_pos,
    }
}
