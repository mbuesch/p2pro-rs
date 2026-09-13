//! An "ironbow" style false-color palette:
//! The classic black -> purple -> red -> orange -> yellow -> white
//! look used by most thermal cameras.

use curveipo::Curve;

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

/// Builds a 256-entry RGBA lookup table spanning the palette, so that
/// mapping a normalized temperature (0..=255) to a color is a plain index.
pub fn build_color_lut() -> [[u8; 4]; 256] {
    let red = Curve::new(STOPS.map(|(p, [r, _, _])| (p, r as f32)));
    let green = Curve::new(STOPS.map(|(p, [_, g, _])| (p, g as f32)));
    let blue = Curve::new(STOPS.map(|(p, [_, _, b])| (p, b as f32)));

    let mut lut = [[0u8; 4]; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let t = i as f32 / 255.0;
        *entry = [
            red.lin_inter(t).round().clamp(0.0, 255.0) as u8,
            green.lin_inter(t).round().clamp(0.0, 255.0) as u8,
            blue.lin_inter(t).round().clamp(0.0, 255.0) as u8,
            255,
        ];
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
