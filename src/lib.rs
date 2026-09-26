#[cfg(all(feature = "rusb", feature = "nusb"))]
std::compile_error!("Features 'rusb' and 'nusb' cannot be enabled at the same time.");
#[cfg(not(any(feature = "rusb", feature = "nusb")))]
std::compile_error!("Either feature 'rusb' or 'nusb' must be enabled.");
#[cfg(all(not(feature = "rusb"), target_os = "android"))]
std::compile_error!("Feature 'rusb' must be enabled for Android builds.");

mod app;
mod camera;
mod colormap;
mod render;
mod save;
mod util;
#[doc(hidden)]
pub mod video;

pub use app::{App, FromUi};
pub use camera::Camera;
