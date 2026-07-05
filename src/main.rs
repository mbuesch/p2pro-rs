use crate::camera::Camera;
use clap::Parser;
use dioxus::desktop::{Config, WindowBuilder};
use std::{path::PathBuf, sync::Arc};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task,
};

mod app;
mod camera;
mod colormap;
mod render;

fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let bytes = include_bytes!("../assets/icon-64x64.png");
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

#[derive(Parser)]
struct Args {
    /// Path to the p2pro camera device (e.g. `/dev/video2`).
    ///
    /// If not specified, all existing /dev/video* devices will be probed
    /// and the first found p2pro device will be used.
    device: Option<PathBuf>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = Args::parse();

    let (tx, rx) = mpsc::channel(32);

    task::spawn({
        let device_path = args.device;
        async move { Camera::capture_loop(device_path.as_deref(), tx).await }
    });

    let window = WindowBuilder::new()
        .with_always_on_top(false)
        .with_title("InfiRay P2Pro")
        .with_window_icon(load_window_icon());
    let config = Config::new().with_window(window).with_menu(None);
    let builder = dioxus::LaunchBuilder::desktop();

    tokio::task::unconstrained({
        let rx = Arc::new(AsyncMutex::new(rx));
        async move {
            builder.with_cfg(config).with_context(rx).launch(app::App);
        }
    })
    .await;
}
