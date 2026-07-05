//! Turns a temperature grid into a false-color PNG (as a data: URI) plus the
//! min/max statistics needed to draw markers and the legend.

use crate::colormap::build_color_lut;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use movavg::MovAvg;

/// Number of frames over which to smooth the min/max temperature values.
const MINMAX_TEMP_SMOOTHING: usize = 30;

/// Result of rendering one frame: a ready-to-embed PNG data URI, the
/// min/max temperatures found, and their pixel coordinates.
pub struct RenderedFrame {
    pub data_uri: String,
    pub min_temp: f32,
    pub max_temp: f32,
    pub min_pos: (u32, u32),
    pub max_pos: (u32, u32),
}

pub struct Renderer {
    color_lut: [[u8; 4]; 256],
    min_temp: MovAvg<f32, f32, MINMAX_TEMP_SMOOTHING>,
    max_temp: MovAvg<f32, f32, MINMAX_TEMP_SMOOTHING>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            color_lut: build_color_lut(),
            min_temp: MovAvg::new(),
            max_temp: MovAvg::new(),
        }
    }

    /// Maps `temps` (row-major, `width` x `height`) through `lut` after
    /// auto-scaling to the frame's own min/max, and encodes the result as PNG.
    pub fn build_frame(&mut self, width: u32, height: u32, temps: &[f32]) -> RenderedFrame {
        let mut min_temp = f32::MAX;
        let mut max_temp = f32::MIN;
        let mut min_pos = (0, 0);
        let mut max_pos = (0, 0);

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

        // Smooth the min/max over the last N frames.
        min_temp = self.min_temp.feed(min_temp);
        max_temp = self.max_temp.feed(max_temp);

        // Auto-scale: the color range always spans exactly this frame's min/max.
        let range = (max_temp - min_temp).max(0.1);

        // Convert to RGBA8 using the color LUT.
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for t in temps {
            let n = (((t - min_temp) / range) * 255.0).clamp(0.0, 255.0) as usize;
            rgba.extend(&self.color_lut[n]);
        }

        // Encode as data URI for embedding in HTML.
        let mut png_bytes = Vec::with_capacity(1024 * 512);
        PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Fast, FilterType::Sub)
            .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
            .expect("encoding a thermal frame to PNG should never fail");
        let mut data_uri = String::with_capacity(png_bytes.len() * 2);
        data_uri.push_str("data:image/png;base64,");
        STANDARD.encode_string(&png_bytes, &mut data_uri);

        RenderedFrame {
            data_uri,
            min_temp,
            max_temp,
            min_pos,
            max_pos,
        }
    }
}
