use crate::camera::CaptureState;
use clap::Parser;
use std::sync::{Arc, Mutex};

mod app;
mod camera;
mod colormap;
mod render;

#[derive(Parser)]
struct Args {
    device: String,
}

fn main() {
    let args = Args::parse();
    let device_path = args.device;

    let shared: Arc<Mutex<CaptureState>> = Arc::new(Mutex::new(CaptureState::Connecting));

    std::thread::spawn({
        let shared = shared.clone();
        move || camera::capture_loop(device_path, shared)
    });

    dioxus::LaunchBuilder::new()
        .with_context(shared)
        .launch(app::App);
}
