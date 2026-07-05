use crate::camera::{Camera, CaptureState};
use clap::Parser;
use tokio::sync::watch;

mod app;
mod camera;
mod colormap;
mod render;

#[derive(Parser)]
struct Args {
    /// Path to the p2pro camera device (e.g. `/dev/video2`).
    device: String,
}

fn main() {
    let args = Args::parse();

    let (tx, rx) = watch::channel(CaptureState::Connecting);

    std::thread::spawn({
        let device_path = args.device;
        move || Camera::capture_loop(device_path, tx)
    });

    dioxus::LaunchBuilder::new()
        .with_context(rx)
        .launch(app::App);
}
