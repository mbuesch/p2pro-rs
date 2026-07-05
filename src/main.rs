use crate::camera::CaptureState;
use clap::Parser;
use tokio::sync::watch;

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

    let (tx, rx) = watch::channel(CaptureState::Connecting);

    std::thread::spawn(move || camera::capture_loop(device_path, tx));

    dioxus::LaunchBuilder::new()
        .with_context(rx)
        .launch(app::App);
}
