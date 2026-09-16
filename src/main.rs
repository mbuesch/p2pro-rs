#[cfg(not(any(target_os = "linux", target_os = "android")))]
std::compile_error!("p2pro-rs is only supported on Linux and Android platforms.");

use crate::{app::FromUi, camera::Camera};
use std::{path::PathBuf, sync::Arc};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc, watch},
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
    let (header, rgba) =
        include_bytes!(concat!(env!("OUT_DIR"), "/icon.rgba")).split_at_checked(4 + 4)?;
    let width = u32::from_le_bytes(header[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(header[4..8].try_into().ok()?);
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.to_vec(), width, height).ok()
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
    /// Path to the InfiRay P2Pro camera device (e.g. `/dev/video2`).
    ///
    /// If not specified, all existing /dev/video* devices will be probed
    /// and the first found P2Pro device will be used.
    device: Option<PathBuf>,

    /// Generate an animated test picture instead of using a real hardware camera.
    #[arg(long, short = 'd')]
    demo: bool,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    init_logging();

    #[cfg(target_os = "android")]
    let (device_path, demo): (Option<PathBuf>, bool) = (None, false);
    #[cfg(not(target_os = "android"))]
    let (device_path, demo) = {
        let args = Args::parse();
        (args.device, args.demo)
    };

    let (to_ui_tx, to_ui_rx) = mpsc::channel(32);
    let (from_ui_tx, from_ui_rx) = watch::channel(FromUi::default());

    task::spawn(async move {
        Camera::capture_loop(device_path.as_deref(), demo, to_ui_tx, from_ui_rx).await
    });

    #[cfg(target_os = "android")]
    let builder = dioxus::LaunchBuilder::mobile();

    #[cfg(not(target_os = "android"))]
    let builder = {
        let window = WindowBuilder::new()
            .with_always_on_top(false)
            .with_title("InfiRay P2Pro Rs")
            .with_window_icon(load_window_icon());
        let config = Config::new().with_window(window).with_menu(None);
        dioxus::LaunchBuilder::desktop().with_cfg(config)
    };

    tokio::task::unconstrained({
        let to_ui_rx = Arc::new(AsyncMutex::new(to_ui_rx));
        async move {
            builder
                .with_context(to_ui_rx)
                .with_context(from_ui_tx)
                .launch(app::App);
        }
    })
    .await;
}
