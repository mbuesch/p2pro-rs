//! The Dioxus UI: a live false-color thermal view, a min/max marker overlay,
//! and a color-scale legend. See `camera.rs` for the capture thread that
//! feeds this UI through shared state.

use crate::{
    camera::{CaptureState, ThermalFrame},
    colormap,
};
use dioxus::prelude::*;
use tokio::sync::watch;

const CSS: &str = include_str!("style.css");

#[component]
pub fn App() -> Element {
    let shared = use_context::<watch::Receiver<CaptureState>>();
    let mut state = use_signal(|| CaptureState::Connecting);

    // Long-lived background task:
    // Waits on the capture thread's `watch` channel and re-renders the UI as soon as it changes.
    use_hook(|| {
        let mut shared = shared.clone();
        spawn(async move {
            loop {
                let snapshot = shared.borrow_and_update().clone();
                state.set(snapshot);
                if let Err(e) = shared.changed().await {
                    eprintln!("Error: Capture thread has exited: {e}");
                    break;
                }
            }
        })
    });

    let current = state();

    rsx! {
        style { "{CSS}" }
        div { id: "app",
            h1 { "P2Pro" }
            match current {
                CaptureState::Connecting => rsx! {
                    p { class: "status", "Connecting to camera..." }
                },
                CaptureState::Error(msg) => rsx! {
                    p { class: "status error", "{msg}" }
                },
                CaptureState::Frame(frame) => rsx! {
                    ThermalView { frame }
                },
            }
        }
    }
}

#[component]
fn ThermalView(frame: ThermalFrame) -> Element {
    let min_left = pct(frame.min_pos.0, frame.width);
    let min_top = pct(frame.min_pos.1, frame.height);
    let max_left = pct(frame.max_pos.0, frame.width);
    let max_top = pct(frame.max_pos.1, frame.height);
    let gradient = colormap::css_gradient();

    rsx! {
        div { class: "viewer",
            div { class: "image-wrap",
                img { class: "thermal-img", src: "{frame.data_uri}" }
                div {
                    class: "marker marker-min",
                    style: "left: {min_left}%; top: {min_top}%;",
                    span { class: "dot" }
                    span { class: "label", "{frame.min_temp:.1}\u{00b0}C" }
                }
                div {
                    class: "marker marker-max",
                    style: "left: {max_left}%; top: {max_top}%;",
                    span { class: "dot" }
                    span { class: "label", "{frame.max_temp:.1}\u{00b0}C" }
                }
            }
            div { class: "legend",
                div { class: "legend-bar", style: "background: {gradient};" }
                div { class: "legend-labels",
                    span { "{frame.max_temp:.1}\u{00b0}C" }
                    span { "{frame.min_temp:.1}\u{00b0}C" }
                }
            }
        }
    }
}

/// Percentage position of pixel coordinate `v` along an axis of `total`
/// pixels, for placing a marker over the (CSS-scaled) image.
fn pct(v: u32, total: u32) -> f32 {
    if total <= 1 {
        0.0
    } else {
        (v as f32 / (total - 1) as f32 * 100.0).clamp(0.0, 100.0)
    }
}
