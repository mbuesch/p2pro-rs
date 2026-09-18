//! Turns a temperature grid into a false-color PNG (as a data: URI) plus the
//! min/max statistics needed to draw markers and the legend.

use crate::{app::FromUi, colormap::build_color_lut};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use movavg::MovAvg;
use std::sync::Arc;

/// Number of frames over which to smooth the min/max temperature values.
const MINMAX_TEMP_SMOOTHING: usize = 30;

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrameUri {
    pub png_uri: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrameRgba {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrameMeta {
    pub width: u32,
    pub height: u32,
    pub min_temp: f32,
    pub max_temp: f32,
    pub min_pos: (u32, u32),
    pub max_pos: (u32, u32),
    /// Effective scale used for coloring: the manual limit where set,
    /// otherwise the smoothed auto min/max.
    pub scale_min: f32,
    pub scale_max: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrame {
    pub meta: RenderedFrameMeta,
    pub rgba: Arc<RenderedFrameRgba>,
    pub uri: Arc<RenderedFrameUri>,
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

    /// Maps `temps` (row-major, `width` x `height`) through the color LUT and
    /// encodes the result as PNG.
    ///
    /// The scale auto-spans the frame's own smoothed min/max; each side of
    /// `from_ui`, when set, pins that end of the scale to a fixed temperature.
    pub fn build_frame(
        &mut self,
        width: u32,
        height: u32,
        temps: &[f32],
        from_ui: &FromUi,
    ) -> RenderedFrame {
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

        // Effective scale: manual limits replace the auto-scale.
        let scale_min = from_ui.min_temp.unwrap_or(min_temp);
        let scale_max = from_ui.max_temp.unwrap_or(max_temp);
        let range = (scale_max - scale_min).max(0.1);

        // Convert to RGBA8 using the color LUT.
        let mut rgba_bytes = Vec::with_capacity((width * height * 4) as usize);
        for t in temps {
            let n = (((t - scale_min) / range) * 255.0).clamp(0.0, 255.0) as usize;
            rgba_bytes.extend(&self.color_lut[n]);
        }

        // Encode as data URI for embedding in HTML.
        let mut png_bytes = Vec::with_capacity(1024 * 512);
        PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Fast, FilterType::Sub)
            .write_image(&rgba_bytes, width, height, ExtendedColorType::Rgba8)
            .expect("encoding a thermal frame to PNG should never fail");
        let mut png_uri = String::with_capacity(png_bytes.len() * 2);
        png_uri.push_str("data:image/png;base64,");
        STANDARD.encode_string(&png_bytes, &mut png_uri);

        let meta = RenderedFrameMeta {
            width,
            height,
            min_temp,
            max_temp,
            min_pos,
            max_pos,
            scale_min,
            scale_max,
        };
        let rgba = Arc::new(RenderedFrameRgba { bytes: rgba_bytes });
        let uri = Arc::new(RenderedFrameUri { png_uri });
        RenderedFrame { meta, rgba, uri }
    }
}
