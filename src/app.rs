//! The Dioxus UI: a live false-color thermal view, a min/max marker overlay,
//! and a color-scale legend. See `camera.rs` for the capture thread that
//! feeds this UI through shared state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dioxus::prelude::*;

use crate::camera::{CaptureState, ThermalFrame};
use crate::colormap;

const CSS: &str = r#"
:root { color-scheme: dark; }
body {
    margin: 0;
    background: #101114;
    color: #e8e8e8;
    font-family: -apple-system, "Segoe UI", Roboto, sans-serif;
}
#app { padding: 20px; }
h1 { font-size: 1.3rem; font-weight: 600; margin: 0 0 4px 0; }
.subtitle { margin: 0 0 16px 0; color: #9a9a9a; font-size: 0.9rem; }
.status { font-size: 1rem; opacity: 0.85; }
.status.error { color: #ff6b6b; }
.viewer { display: flex; gap: 20px; align-items: flex-start; }
.image-wrap {
    position: relative;
    width: 640px;
    height: 480px;
    background: #000;
    border: 1px solid #333;
}
.thermal-img {
    width: 100%;
    height: 100%;
    display: block;
    image-rendering: pixelated;
}
.marker {
    position: absolute;
    transform: translate(-50%, -50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    pointer-events: none;
}
.marker .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid white;
    box-shadow: 0 0 4px rgba(0, 0, 0, 0.9);
}
.marker-max .dot { background: #ff3b30; }
.marker-min .dot { background: #3b82ff; }
.marker .label {
    margin-top: 3px;
    font-size: 12px;
    font-weight: 600;
    background: rgba(0, 0, 0, 0.6);
    padding: 1px 5px;
    border-radius: 3px;
    white-space: nowrap;
}
.legend { display: flex; flex-direction: row; gap: 8px; }
.legend-bar {
    width: 28px;
    height: 480px;
    border: 1px solid #333;
}
.legend-labels {
    height: 480px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    font-size: 13px;
    padding: 2px 0;
}
"#;

#[component]
pub fn App() -> Element {
    let shared = use_context::<Arc<Mutex<CaptureState>>>();
    let mut state = use_signal(|| CaptureState::Connecting);

    // Long-lived background task: polls the capture thread's shared state
    // and mirrors it into a signal so the UI re-renders when it changes.
    // `App` is the root component and is never unmounted, so a plain
    // `spawn` (owned by this same scope) lives for the whole app run.
    use_hook(|| {
        let shared = shared.clone();
        spawn(async move {
            loop {
                let snapshot = { shared.lock().unwrap().clone() };
                state.set(snapshot);
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        })
    });

    let current = state();

    rsx! {
        style { "{CSS}" }
        div { id: "app",
            h1 { "P2Pro Thermal Viewer" }
            p { class: "subtitle", "false-color view - auto-scaled to the current frame's min/max" }
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
        v as f32 / (total - 1) as f32 * 100.0
    }
}
