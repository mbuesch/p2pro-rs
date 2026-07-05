use crate::camera::Camera;
use clap::Parser;
use std::sync::Arc;
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task,
};

mod app;
mod camera;
mod colormap;
mod render;

#[derive(Parser)]
struct Args {
    /// Path to the p2pro camera device (e.g. `/dev/video2`).
    device: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = Args::parse();

    let (tx, rx) = mpsc::channel(32);

    task::spawn({
        let device_path = args.device;
        async move { Camera::capture_loop(device_path, tx).await }
    });

    let builder = dioxus::LaunchBuilder::desktop();

    tokio::task::unconstrained({
        let rx = Arc::new(AsyncMutex::new(rx));
        async move {
            builder.with_context(rx).launch(app::App);
        }
    })
    .await;
}
