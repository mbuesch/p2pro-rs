//! The Dioxus UI: a live false-color thermal view, a min/max marker overlay,
//! and a color-scale legend. See `camera.rs` for the capture thread that
//! feeds this UI through shared state.

use crate::{
    camera::CaptureState,
    colormap,
    render::RenderedFrame,
    save::{pick_video_target, save_frame_png},
    video::{VideoEvent, VideoRecorder},
};
use dioxus::prelude::*;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};

const CSS: &str = include_str!("style.css");

/// Settings sent from the UI to the capture thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FromUi {
    pub min_temp: Option<f32>,
    pub max_temp: Option<f32>,
}

#[component]
pub fn App() -> Element {
    let from_cam = use_context::<Arc<AsyncMutex<mpsc::Receiver<CaptureState>>>>();
    let recorder = use_context::<VideoRecorder>();
    let mut state = use_signal(|| CaptureState::Connecting);
    let running = use_signal(|| true);

    let video_name = use_signal(|| None::<String>);
    let mut video_err = use_signal(|| None::<String>);
    let mut video_recording = use_signal(|| false);

    use_hook(|| {
        let recorder = recorder.clone();
        spawn(async move {
            let mut from_cam = from_cam.lock().await;
            loop {
                let Some(snapshot) = from_cam.recv().await else {
                    eprintln!("Error: Capture thread has exited");
                    break;
                };
                if let CaptureState::Frame(frame) = &snapshot {
                    if !running() {
                        continue; // While stopped, drop incoming frames.
                    }
                    // Push to frame recorder.
                    recorder.push_frame(frame.width, frame.height, frame.rgba_bytes.clone());
                }
                // To live-view.
                state.set(snapshot);
            }
        })
    });

    use_hook(|| {
        let recorder = recorder.clone();
        spawn(async move {
            while let Some(event) = recorder.next_event().await {
                match event {
                    VideoEvent::Stopped { frames: _ } => {
                        video_recording.set(false);
                    }
                    VideoEvent::Error(msg) => {
                        video_recording.set(false);
                        video_err.set(Some(msg));
                    }
                }
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
                    ThermalView {
                        frame,
                        running,
                        recorder: recorder.clone(),
                        video_name,
                        video_err,
                        video_recording,
                    }
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
fn ThermalView(
    frame: RenderedFrame,
    mut running: Signal<bool>,
    recorder: VideoRecorder,
    mut video_name: Signal<Option<String>>,
    mut video_err: Signal<Option<String>>,
    mut video_recording: Signal<bool>,
) -> Element {
    let gradient = colormap::css_gradient();

    let from_ui_tx = use_context::<watch::Sender<FromUi>>();
    let mut fix_min = use_signal(|| false);
    let mut fix_max = use_signal(|| false);
    let mut min_text = use_signal(String::new);
    let mut max_text = use_signal(String::new);

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

    // Compute marker positions in CSS px inside image-wrap.
    // The markers live outside the scaled surface so their size stays constant.
    let fit_size = (fit_w, fit_h);
    let wrap_origin = (wl, wt);
    let (min_left, min_top) = marker_screen_px(
        frame.min_pos,
        &frame,
        box_center,
        current_pan,
        current_zoom,
        fit_size,
        wrap_origin,
    );
    let (max_left, max_top) = marker_screen_px(
        frame.max_pos,
        &frame,
        box_center,
        current_pan,
        current_zoom,
        fit_size,
        wrap_origin,
    );

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
    let onsavepic = {
        let frame = frame.clone();
        move |_| {
            let frame = frame.clone();
            spawn(async move {
                save_frame_png(&frame).await;
            });
        }
    };

    let onsavevid = {
        let recorder = recorder.clone();
        move |_| {
            if video_recording() {
                video_recording.set(false);
                recorder.stop();
            } else {
                let recorder = recorder.clone();
                spawn(async move {
                    if let Some((name, target)) = pick_video_target().await {
                        recorder.start(target);
                        video_name.set(Some(name));
                        video_recording.set(true);
                        video_err.set(None);
                    }
                });
            }
        }
    };

    let cur_min_temp = frame.min_temp;
    let cur_max_temp = frame.max_temp;

    let on_fix_min = {
        let tx = from_ui_tx.clone();
        move |e: Event<FormData>| {
            let on = e.checked();
            fix_min.set(on);
            if on && min_text().trim().is_empty() {
                min_text.set(format!("{cur_min_temp:.1}"));
            }
            send_from_ui(&tx, (fix_min(), &min_text()), (fix_max(), &max_text()));
        }
    };
    let on_min_input = {
        let tx = from_ui_tx.clone();
        move |e: Event<FormData>| {
            min_text.set(e.value());
            send_from_ui(&tx, (fix_min(), &min_text()), (fix_max(), &max_text()));
        }
    };
    let on_fix_max = {
        let tx = from_ui_tx.clone();
        move |e: Event<FormData>| {
            let on = e.checked();
            fix_max.set(on);
            if on && max_text().trim().is_empty() {
                max_text.set(format!("{cur_max_temp:.1}"));
            }
            send_from_ui(&tx, (fix_min(), &min_text()), (fix_max(), &max_text()));
        }
    };
    let on_max_input = {
        let tx = from_ui_tx.clone();
        move |e: Event<FormData>| {
            max_text.set(e.value());
            send_from_ui(&tx, (fix_min(), &min_text()), (fix_max(), &max_text()));
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
                }
                div {
                    class: "marker marker-min",
                    style: "left: {min_left}px; top: {min_top}px;",
                    span { class: "dot" }
                    span { class: "label", "{frame.min_temp:.1}\u{00b0}C" }
                }
                div {
                    class: "marker marker-max",
                    style: "left: {max_left}px; top: {max_top}px;",
                    span { class: "dot" }
                    span { class: "label", "{frame.max_temp:.1}\u{00b0}C" }
                }
            }
            div { class: "legend",
                div { class: "legend-main",
                    div { class: "legend-bar", style: "background: {gradient};" }
                    div { class: "legend-labels",
                        span { "{frame.scale_max:.1}\u{00b0}C" }
                        span { "{frame.scale_min:.1}\u{00b0}C" }
                    }
                }
                div { class: "range-controls",
                    div { class: "range-row",
                        label { class: "range-toggle",
                            input {
                                r#type: "checkbox",
                                checked: fix_min(),
                                onchange: on_fix_min,
                            }
                            "Min"
                        }
                        input {
                            class: "range-input",
                            r#type: "number",
                            step: "0.5",
                            disabled: !fix_min(),
                            value: "{min_text}",
                            oninput: on_min_input,
                        }
                        span { class: "range-unit", "°C" }
                    }
                    div { class: "range-row",
                        label { class: "range-toggle",
                            input {
                                r#type: "checkbox",
                                checked: fix_max(),
                                onchange: on_fix_max,
                            }
                            "Max"
                        }
                        input {
                            class: "range-input",
                            r#type: "number",
                            step: "0.5",
                            disabled: !fix_max(),
                            value: "{max_text}",
                            oninput: on_max_input,
                        }
                        span { class: "range-unit", "°C" }
                    }
                }
                div { class: "controls",
                    button { class: "control-btn", onclick: onstartstop,
                        if running() {
                            "Stop cam"
                        } else {
                            "Start cam"
                        }
                    }
                    button { class: "control-btn", onclick: onsavepic, "Save pic" }
                    button {
                        class: if video_recording() { if running() { "control-btn rec rec-active" } else { "control-btn rec rec-paused" } } else { "control-btn" },
                        onclick: onsavevid,
                        if video_recording() {
                            if running() {
                                "\u{25a0} Stop"
                            } else {
                                "\u{25a0} Stop (paused)"
                            }
                        } else {
                            "Save vid"
                        }
                    }
                    if video_recording() {
                        if let Some(name) = video_name() {
                            div { class: "video-file", title: "{name}", "{name}" }
                        }
                    }
                    if let Some(err) = video_err() {
                        p { class: "status error", "{err}" }
                    }
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

/// Maps a pixel coordinate in the thermal frame to a CSS `left`/`top`
/// position in `image-wrap` px, accounting for the current zoom and pan.
fn marker_screen_px(
    pos: (u32, u32),
    frame: &RenderedFrame,
    box_center: (f64, f64),
    pan: (f64, f64),
    zoom: f64,
    fit_size: (f64, f64),
    wrap_origin: (f64, f64),
) -> (f64, f64) {
    let nx = if frame.width <= 1 {
        0.0
    } else {
        pos.0 as f64 / (frame.width - 1) as f64
    };
    let ny = if frame.height <= 1 {
        0.0
    } else {
        pos.1 as f64 / (frame.height - 1) as f64
    };
    let img_w = fit_size.0 * zoom;
    let img_h = fit_size.1 * zoom;
    let center = (box_center.0 + pan.0, box_center.1 + pan.1);
    let abs_x = center.0 - img_w / 2.0 + nx * img_w;
    let abs_y = center.1 - img_h / 2.0 + ny * img_h;
    (abs_x - wrap_origin.0, abs_y - wrap_origin.1)
}

/// Parses a temperature text field.
/// Accepts both "." and "," decimals.
/// Empty or invalid text counts as "no manual limit" (auto-scaling).
fn parse_temp(text: &str) -> Option<f32> {
    text.trim().replace(',', ".").parse().ok()
}

/// Sends the current state of the manual-range widgets to the capture thread.
fn send_from_ui(from_ui_tx: &watch::Sender<FromUi>, min: (bool, &str), max: (bool, &str)) {
    let _ = from_ui_tx.send(FromUi {
        min_temp: if min.0 { parse_temp(min.1) } else { None },
        max_temp: if max.0 { parse_temp(max.1) } else { None },
    });
}
