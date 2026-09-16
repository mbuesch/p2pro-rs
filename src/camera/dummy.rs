//! Dummy camera backend for testing without hardware.
//!
//! Generates an animated synthetic thermal image.

use super::{CaptureState, HEIGHT, WIDTH, decode_frame};
use crate::{app::FromUi, render::Renderer};
use std::{
    f32::consts::PI,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

/// Frame interval of the dummy camera.
const FRAME_INTERVAL: Duration = Duration::from_millis(40);

/// Runs forever: generates animated test frames and streams them into `to_ui`.
pub async fn capture_loop(to_ui: mpsc::Sender<CaptureState>, from_ui: watch::Receiver<FromUi>) {
    println!("Test mode: using dummy camera (no hardware)");
    let _ = to_ui
        .send(CaptureState::Info(
            "Test mode: using dummy camera (no hardware)".to_string(),
        ))
        .await;

    let renderer = Mutex::new(Renderer::new());
    let start = Instant::now();
    let stride = (WIDTH * 2) as usize;
    let mut buf = vec![0u8; stride * (HEIGHT * 2) as usize];

    loop {
        fill_frame(&mut buf, stride, start.elapsed().as_secs_f32());

        let frame = {
            let from_ui = from_ui.borrow().clone();
            let mut renderer = renderer.lock().expect("Lock poisoned");
            decode_frame(&mut renderer, &buf, stride, &from_ui)
        };
        if let Some(frame) = frame {
            let _ = to_ui.send(CaptureState::Frame(frame)).await;
        }

        tokio::time::sleep(FRAME_INTERVAL).await;
    }
}

/// Fills the thermal half of `buf` (dummy YUYV frame, `stride` bytes per row)
/// with an animated test pattern at time `t`.
/// The top (video) half is left black.
fn fill_frame(buf: &mut [u8], stride: usize, t: f32) {
    let width = WIDTH as usize;
    let height = HEIGHT as usize;
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;

    // The hot spot orbits the image center, the cold spot moves in opposition.
    let hot_x = cx + (t * 0.9).cos() * cx * 0.6;
    let hot_y = cy + (t * 0.9).sin() * cy * 0.6;
    let cold_x = cx + (t * 0.5 + PI).cos() * cx * 0.7;
    let cold_y = cy + (t * 0.5 + PI).sin() * cy * 0.7;

    for y in 0..height {
        let row = height + y; // bottom half carries the raw thermal data
        let row_start = row * stride;
        for x in 0..width {
            // Slowly "breathing" background gradient, ~20-27 C.
            let mut temp = 20.0 + 5.0 * (x as f32 / width as f32) + 2.0 * (t * 0.7).sin();

            // Gaussian hot spot, up to ~+40 C at its center.
            let d2_hot = (x as f32 - hot_x).powi(2) + (y as f32 - hot_y).powi(2);
            temp += 40.0 * (-d2_hot / 400.0).exp();

            // Gaussian cold spot, down to ~-15 C at its center.
            let d2_cold = (x as f32 - cold_x).powi(2) + (y as f32 - cold_y).powi(2);
            temp -= 15.0 * (-d2_cold / 900.0).exp();

            // Encode like the real camera: raw = (C + 273.2) * 64, LE u16.
            let raw = ((temp + 273.2) * 64.0).clamp(0.0, u16::MAX as f32) as u16;
            let offset = row_start + x * 2;
            buf[offset] = raw as u8;
            buf[offset + 1] = (raw >> 8) as u8;
        }
    }
}
