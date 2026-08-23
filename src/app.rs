//! The Dioxus UI: a live false-color thermal view, a min/max marker overlay,
//! and a color-scale legend. See `camera.rs` for the capture thread that
//! feeds this UI through shared state.

use crate::{camera::CaptureState, colormap, render::RenderedFrame, save::save_frame_png};
use dioxus::prelude::*;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

const CSS: &str = include_str!("style.css");

#[component]
pub fn App() -> Element {
    let from_cam = use_context::<Arc<AsyncMutex<mpsc::Receiver<CaptureState>>>>();
    let mut state = use_signal(|| CaptureState::Connecting);
    let running = use_signal(|| true);

    // Long-lived background task:
    // Waits on the capture thread's `mpsc` channel and re-renders the UI as soon as it changes.
    use_hook(|| {
        spawn(async move {
            let mut from_cam = from_cam.lock().await;
            loop {
                let Some(snapshot) = from_cam.recv().await else {
                    eprintln!("Error: Capture thread has exited");
                    break;
                };
                if !running() && matches!(snapshot, CaptureState::Frame(_)) {
                    continue; // While stopped, drop incoming frames.
                }
                state.set(snapshot);
            }
        })
    });

    let current = state();

    rsx! {
        style { "{CSS}" }
        div { id: "app",
            h1 { "P2Pro - Thermal cam" }
            match current {
                CaptureState::Connecting => rsx! {
                    p { class: "status", "Connecting to camera..." }
                },
                ref c @ CaptureState::Info(ref msg) | ref c @ CaptureState::Error(ref msg) => {
                    rsx! {
                        p { class: if matches!(c, CaptureState::Info(_)) { "status info" } else { "status error" },
                            for (i, line) in msg.split('\n').enumerate() {
                                if i > 0 {
                                    br {}
                                }
                                "{line}"
                            }
                        }
                    }
                }
                CaptureState::Frame(frame) => rsx! {
                    ThermalView { frame, running }
                },
            }
        }
    }
}

/// Zoom is always >= this: 1.0 means the picture exactly fits the available screen area.
const MIN_ZOOM: f64 = 1.0;
const MAX_ZOOM: f64 = 6.0;
/// Zoom multiplier applied per mouse wheel click.
const WHEEL_ZOOM_STEP: f64 = 1.15;

/// One actively-touching pointer (mouse button held, or a finger on screen),
#[derive(Clone, Copy)]
struct TrackedPointer {
    id: i32,
    x: f64,
    y: f64,
}

/// Snapshot taken at the start of a drag/pinch gesture.
#[derive(Clone, Copy)]
struct GestureBaseline {
    anchor: (f64, f64),
    zoom0: f64,
    dist0: f64,
}

#[component]
fn ThermalView(frame: RenderedFrame, mut running: Signal<bool>) -> Element {
    let min_left = percent(frame.min_pos.0, frame.width);
    let min_top = percent(frame.min_pos.1, frame.height);
    let max_left = percent(frame.max_pos.0, frame.width);
    let max_top = percent(frame.max_pos.1, frame.height);
    let gradient = colormap::css_gradient();

    let mut zoom = use_signal(|| MIN_ZOOM);
    let mut pan = use_signal(|| (0.0_f64, 0.0_f64));
    let mut wrap_rect = use_signal(|| (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64));
    let mut pointers = use_signal(Vec::<TrackedPointer>::new);
    let mut gesture = use_signal(|| None::<GestureBaseline>);

    use_hook(|| {
        spawn(async move {
            let mut eval = document::eval(
                r#"
                const el = document.getElementById('image-wrap');
                function send() {
                    if (!el) return;
                    const r = el.getBoundingClientRect();
                    dioxus.send([r.left, r.top, r.width, r.height]);
                }
                window.addEventListener('resize', send);
                window.addEventListener('orientationchange', send);
                send();
                "#,
            );
            // The screen changed shape (e.g. horiz/vert tilt) - re-fit the
            // picture to it and forget any manual zoom/pan.
            while let Ok(rect) = eval.recv::<(f64, f64, f64, f64)>().await {
                wrap_rect.set(rect);
                zoom.set(MIN_ZOOM);
                pan.set((0.0, 0.0));
                pointers.set(vec![]);
                gesture.set(None);
            }
        });
    });

    let (wl, wt, ww, wh) = wrap_rect();
    let box_center = (wl + ww / 2.0, wt + wh / 2.0);

    // Largest size (in CSS px) that fits the frame's aspect ratio
    // inside the measured wrap (zoom == 1.0).
    let (fit_w, fit_h) = if ww > 0.0 && wh > 0.0 {
        let ar = frame.width as f64 / frame.height as f64;
        if ww / wh > ar {
            (wh * ar, wh)
        } else {
            (ww, ww / ar)
        }
    } else {
        (frame.width as f64, frame.height as f64)
    };

    let current_zoom = zoom();
    let current_pan = pan();
    let is_panning = !pointers().is_empty();

    let onpointerdown = move |evt: Event<PointerData>| {
        evt.prevent_default();
        let c = evt.client_coordinates();
        let id = evt.pointer_id();
        let mut pts = pointers();
        pts.retain(|p| p.id != id);
        if pts.len() < 2 {
            pts.push(TrackedPointer { id, x: c.x, y: c.y });
        }
        gesture.set(Some(make_gesture(&pts, box_center, zoom(), pan())));
        pointers.set(pts);
    };

    let onpointermove = move |evt: Event<PointerData>| {
        let id = evt.pointer_id();
        let c = evt.client_coordinates();
        let mut pts = pointers();
        let Some(p) = pts.iter_mut().find(|p| p.id == id) else {
            return; // hover move without a matching pointerdown - ignore
        };
        p.x = c.x;
        p.y = c.y;
        if let Some(g) = gesture() {
            let (nz, np) = apply_gesture(&g, &pts, box_center);
            let np = clamp_pan(np, nz, fit_w, fit_h, ww, wh);
            zoom.set(nz);
            pan.set(np);
        }
        pointers.set(pts);
    };

    let mut release_pointer = move |id: i32| {
        let mut pts = pointers();
        pts.retain(|p| p.id != id);
        if pts.is_empty() {
            gesture.set(None);
        } else {
            gesture.set(Some(make_gesture(&pts, box_center, zoom(), pan())));
        }
        pointers.set(pts);
    };
    let onpointerup = move |evt: Event<PointerData>| release_pointer(evt.pointer_id());
    let onpointercancel = move |evt: Event<PointerData>| release_pointer(evt.pointer_id());

    let onwheel = move |evt: Event<WheelData>| {
        evt.prevent_default();
        let dy = evt.delta().strip_units().y;
        if dy == 0.0 {
            return;
        }
        let c = evt.client_coordinates();
        let r = (c.x - box_center.0, c.y - box_center.1);
        let factor = if dy > 0.0 {
            1.0 / WHEEL_ZOOM_STEP
        } else {
            WHEEL_ZOOM_STEP
        };
        let z0 = zoom();
        let p0 = pan();
        let anchor = ((r.0 - p0.0) / z0, (r.1 - p0.1) / z0);
        let nz = (z0 * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let np = (r.0 - nz * anchor.0, r.1 - nz * anchor.1);
        let np = clamp_pan(np, nz, fit_w, fit_h, ww, wh);
        zoom.set(nz);
        pan.set(np);
    };

    let onstartstop = move |_| running.set(!running());
    let onsave = {
        let frame = frame.clone();
        move |_| {
            let frame = frame.clone();
            spawn(async move {
                save_frame_png(&frame).await;
            });
        }
    };

    let surface_style = format!(
        "width: {fit_w}px; height: {fit_h}px; margin-left: {}px; margin-top: {}px; transform: translate({}px, {}px) scale({current_zoom});",
        -fit_w / 2.0,
        -fit_h / 2.0,
        current_pan.0,
        current_pan.1,
    );

    rsx! {
        div { class: "viewer",
            div {
                id: "image-wrap",
                class: if is_panning { "image-wrap panning" } else { "image-wrap" },
                onpointerdown,
                onpointermove,
                onpointerup,
                onpointercancel,
                onwheel,
                div { class: "image-surface", style: "{surface_style}",
                    img { class: "thermal-img", src: "{frame.png_uri}" }
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
            }
            div { class: "legend",
                div { class: "legend-main",
                    div { class: "legend-bar", style: "background: {gradient};" }
                    div { class: "legend-labels",
                        span { "{frame.max_temp:.1}\u{00b0}C" }
                        span { "{frame.min_temp:.1}\u{00b0}C" }
                    }
                }
                div { class: "controls",
                    button { class: "control-btn", onclick: onstartstop,
                        if running() {
                            "Stop"
                        } else {
                            "Start"
                        }
                    }
                    button { class: "control-btn", onclick: onsave, "Save" }
                }
            }
        }
    }
}

/// Recomputes the gesture anchor
/// (the content point that must stay fixed under the pointer(s))
/// from the current zoom/pan and active pointers.
fn make_gesture(
    pts: &[TrackedPointer],
    box_center: (f64, f64),
    zoom0: f64,
    pan0: (f64, f64),
) -> GestureBaseline {
    let r0 = reference_point(pts, box_center);
    let anchor = ((r0.0 - pan0.0) / zoom0, (r0.1 - pan0.1) / zoom0);
    let dist0 = if pts.len() == 2 {
        pointer_distance(pts[0], pts[1])
    } else {
        1.0
    };
    GestureBaseline {
        anchor,
        zoom0,
        dist0,
    }
}

/// Computes the new (zoom, pan) so the gesture's anchor content point stays
/// under the current pointer(s) - single pointer pans, two pointers pinch-zoom.
fn apply_gesture(
    g: &GestureBaseline,
    pts: &[TrackedPointer],
    box_center: (f64, f64),
) -> (f64, (f64, f64)) {
    let rn = reference_point(pts, box_center);
    let new_zoom = if pts.len() == 2 && g.dist0 > 0.0 {
        let distn = pointer_distance(pts[0], pts[1]);
        g.zoom0 * (distn / g.dist0)
    } else {
        g.zoom0
    }
    .clamp(MIN_ZOOM, MAX_ZOOM);
    let new_pan = (rn.0 - new_zoom * g.anchor.0, rn.1 - new_zoom * g.anchor.1);
    (new_zoom, new_pan)
}

/// Midpoint of the active pointers, relative to the image-wrap's center.
fn reference_point(pts: &[TrackedPointer], box_center: (f64, f64)) -> (f64, f64) {
    let n = pts.len().max(1) as f64;
    let sx: f64 = pts.iter().map(|p| p.x).sum();
    let sy: f64 = pts.iter().map(|p| p.y).sum();
    (sx / n - box_center.0, sy / n - box_center.1)
}

fn pointer_distance(a: TrackedPointer, b: TrackedPointer) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Keeps the zoomed picture from being panned so far that it leaves
/// empty space inside the wrap.
fn clamp_pan(
    pan: (f64, f64),
    zoom: f64,
    fit_w: f64,
    fit_h: f64,
    wrap_w: f64,
    wrap_h: f64,
) -> (f64, f64) {
    let max_x = ((fit_w * zoom - wrap_w) / 2.0).max(0.0);
    let max_y = ((fit_h * zoom - wrap_h) / 2.0).max(0.0);
    (pan.0.clamp(-max_x, max_x), pan.1.clamp(-max_y, max_y))
}

/// Percentage position of pixel coordinate `v` along an axis of `total`
/// pixels, for placing a marker over the (CSS-scaled) image.
fn percent(v: u32, total: u32) -> f32 {
    if total <= 1 {
        0.0
    } else {
        (v as f32 / (total - 1) as f32 * 100.0).clamp(0.0, 100.0)
    }
}
