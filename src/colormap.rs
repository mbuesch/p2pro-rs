//! A small hand-rolled "ironbow" style false-color palette, the classic
//! black -> purple -> red -> orange -> yellow -> white look used by most
//! thermal cameras. No external color-gradient crate needed: it's just a
//! handful of RGB stops that we linearly interpolate between.

/// (position in [0,1], RGB color) stops, sorted by position.
const STOPS: [(f32, [u8; 3]); 8] = [
    (0.00, [0, 0, 0]),
    (0.13, [30, 0, 60]),
    (0.28, [90, 0, 140]),
    (0.45, [180, 0, 150]),
    (0.60, [225, 30, 35]),
    (0.75, [248, 150, 10]),
    (0.88, [255, 220, 0]),
    (1.00, [255, 255, 255]),
];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

/// Builds a 256-entry RGBA lookup table spanning the palette, so that
/// mapping a normalized temperature (0..=255) to a color is a plain index.
pub fn build_color_lut() -> [[u8; 4]; 256] {
    let mut lut = [[0u8; 4]; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let t = i as f32 / 255.0;
        let mut color = STOPS[STOPS.len() - 1].1;
        for pair in STOPS.windows(2) {
            let (t0, c0) = pair[0];
            let (t1, c1) = pair[1];
            if t <= t1 {
                let seg = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
                let seg = seg.clamp(0.0, 1.0);
                color = [
                    lerp_u8(c0[0], c1[0], seg),
                    lerp_u8(c0[1], c1[1], seg),
                    lerp_u8(c0[2], c1[2], seg),
                ];
                break;
            }
        }
        *entry = [color[0], color[1], color[2], 255];
    }
    lut
}

/// Renders the same palette as a CSS `linear-gradient`, hottest color on
/// top, for the on-screen legend bar.
pub fn css_gradient() -> String {
    let stops: Vec<String> = STOPS
        .iter()
        .map(|(t, [r, g, b])| format!("#{r:02x}{g:02x}{b:02x} {:.0}%", t * 100.0))
        .collect();
    format!("linear-gradient(to top, {})", stops.join(", "))
}
