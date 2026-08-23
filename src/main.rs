use crate::camera::Camera;
use std::{path::PathBuf, sync::Arc};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task,
};

#[cfg(not(target_os = "android"))]
use clap::Parser;
#[cfg(not(target_os = "android"))]
use dioxus::desktop::{Config, WindowBuilder};

mod app;
mod camera;
mod colormap;
mod render;
mod save;

#[cfg(not(target_os = "android"))]
fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let bytes = include_bytes!("../assets/icon-64x64.png");
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

#[cfg(target_os = "android")]
fn init_logging() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("p2pro"),
    );
}

#[cfg(not(target_os = "android"))]
fn init_logging() {}

#[cfg(not(target_os = "android"))]
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
    init_logging();

    #[cfg(target_os = "android")]
    let device_path: Option<PathBuf> = None;
    #[cfg(not(target_os = "android"))]
    let device_path = Args::parse().device;

    let (tx, rx) = mpsc::channel(32);

    task::spawn(async move { Camera::capture_loop(device_path.as_deref(), tx).await });

    #[cfg(target_os = "android")]
    let builder = dioxus::LaunchBuilder::mobile();

    #[cfg(not(target_os = "android"))]
    let builder = {
        let window = WindowBuilder::new()
            .with_always_on_top(false)
            .with_title("InfiRay P2Pro")
            .with_window_icon(load_window_icon());
        let config = Config::new().with_window(window).with_menu(None);
        dioxus::LaunchBuilder::desktop().with_cfg(config)
    };

    tokio::task::unconstrained({
        let rx = Arc::new(AsyncMutex::new(rx));
        async move {
            builder.with_context(rx).launch(app::App);
        }
    })
    .await;
}
