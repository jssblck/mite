use windows::Win32::Foundation::COLORREF;

use crate::hover::WordCategory;
use crate::hud::Stage;

/// An 8-bit RGB colour. Kept distinct from `COLORREF` (which packs the channels
/// as BGR) so palette entries can be written and destructured as plain (r, g, b).
#[derive(Clone, Copy)]
pub(super) struct Rgb {
    pub(super) r: u8,
    pub(super) g: u8,
    pub(super) b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub(super) fn to_colorref(self) -> COLORREF {
        COLORREF((self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16))
    }
}

// Popup palette. Centralized so the overlay's colour scheme is tunable in one
// place.
pub(super) const COLOR_PANEL_BG: Rgb = Rgb::new(26, 26, 32);
pub(super) const COLOR_PANEL_BORDER: Rgb = Rgb::new(70, 70, 86);
pub(super) const COLOR_WORD_TEXT: Rgb = Rgb::new(245, 245, 250);
pub(super) const COLOR_FURIGANA: Rgb = Rgb::new(150, 200, 255);
pub(super) const COLOR_NOTE_TEXT: Rgb = Rgb::new(205, 175, 95);
pub(super) const COLOR_GLOSS_TEXT: Rgb = Rgb::new(214, 214, 222);
pub(super) const COLOR_BUTTON_BG: Rgb = Rgb::new(46, 46, 58);
pub(super) const COLOR_BUTTON_BORDER: Rgb = Rgb::new(120, 120, 150);
pub(super) const COLOR_BUTTON_ICON: Rgb = Rgb::new(228, 228, 238);
pub(super) const COLOR_HUD_BG: Rgb = Rgb::new(18, 18, 24);
pub(super) const COLOR_HUD_TITLE: Rgb = Rgb::new(205, 205, 215);
pub(super) const COLOR_HUD_STATUS: Rgb = Rgb::new(150, 180, 210);
pub(super) const COLOR_HUD_GRAPH_BG: Rgb = Rgb::new(10, 10, 14);
pub(super) const COLOR_HUD_GRIDLINE: Rgb = Rgb::new(38, 38, 48);
pub(super) const COLOR_HUD_LEGEND_HEADER: Rgb = Rgb::new(140, 140, 152);

/// Highlight colour for a word category.
///
/// The six content categories are separated by lightness as well as hue so they
/// stay distinguishable under common colour-vision deficiencies: the hue wheel
/// collapses under protan/deutan/tritan vision, but lightness survives. Every
/// pair of content colours keeps a perceptual gap of >=10 dE2000 across normal
/// plus all three CVD types (guarded by `tests::category_palette_is_cvd_safe`);
/// the muted tones are the cost of that robustness. The three greys are a
/// deliberate de-emphasis family on fixed light/mid/faint lightness tiers.
pub(super) fn category_rgb(category: WordCategory) -> Rgb {
    match category {
        WordCategory::Particle => Rgb::new(216, 191, 95), // amber
        WordCategory::Noun => Rgb::new(26, 117, 218),     // blue
        WordCategory::Verb => Rgb::new(133, 180, 126),    // sage green
        WordCategory::Adjective => Rgb::new(183, 107, 41), // burnt orange
        WordCategory::Adverb => Rgb::new(131, 94, 147),   // mauve
        WordCategory::Expression => Rgb::new(0, 170, 186), // teal
        WordCategory::Auxiliary => Rgb::new(133, 134, 138), // grey (mid)
        WordCategory::Other => Rgb::new(195, 196, 200),   // grey (light)
        WordCategory::Unknown => Rgb::new(104, 104, 109), // grey (faint)
    }
}

/// Line/legend colour for a HUD stage.
pub(super) fn hud_stage_rgb(stage: Stage) -> Rgb {
    match stage {
        Stage::Total => Rgb::new(245, 245, 250), // white (drawn thickest)
        Stage::Capture => Rgb::new(235, 195, 50), // amber
        Stage::Detect => Rgb::new(90, 200, 120), // green
        Stage::Recognize => Rgb::new(70, 150, 245), // blue
        Stage::Analyze => Rgb::new(190, 130, 235), // purple
        Stage::Present => Rgb::new(80, 200, 200), // teal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Perceptual-colour helpers, kept in the test module so they never touch the
    // hot draw path. They let us assert that the category palette stays legible
    // and colour-vision-deficiency (CVD) safe, rather than trusting hand-tuned
    // RGB triples not to drift on the next edit.

    fn srgb_to_lin(c: f64) -> f64 {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn lin_to_byte(c: f64) -> f64 {
        let v = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055
        };
        (v * 255.0).clamp(0.0, 255.0)
    }

    fn rel_luminance(c: Rgb) -> f64 {
        0.2126 * srgb_to_lin(c.r as f64)
            + 0.7152 * srgb_to_lin(c.g as f64)
            + 0.0722 * srgb_to_lin(c.b as f64)
    }

    /// WCAG contrast ratio between two colours.
    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (la, lb) = (rel_luminance(a), rel_luminance(b));
        let (hi, lo) = (la.max(lb), la.min(lb));
        (hi + 0.05) / (lo + 0.05)
    }

    fn rgb_to_lab(c: Rgb) -> [f64; 3] {
        let (r, g, b) = (
            srgb_to_lin(c.r as f64),
            srgb_to_lin(c.g as f64),
            srgb_to_lin(c.b as f64),
        );
        let x = (r * 0.4124 + g * 0.3576 + b * 0.1805) / 0.95047;
        let y = r * 0.2126 + g * 0.7152 + b * 0.0722;
        let z = (r * 0.0193 + g * 0.1192 + b * 0.9505) / 1.08883;
        let f = |t: f64| {
            if t > 0.008856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    /// CIEDE2000 perceptual colour difference between two CIELAB colours.
    fn ciede2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
        let (l1, a1, b1) = (l1[0], l1[1], l1[2]);
        let (l2, a2, b2) = (l2[0], l2[1], l2[2]);
        let avg_lp = (l1 + l2) / 2.0;
        let c1 = (a1 * a1 + b1 * b1).sqrt();
        let c2 = (a2 * a2 + b2 * b2).sqrt();
        let avg_c = (c1 + c2) / 2.0;
        let g = 0.5 * (1.0 - (avg_c.powi(7) / (avg_c.powi(7) + 25f64.powi(7))).sqrt());
        let a1p = a1 * (1.0 + g);
        let a2p = a2 * (1.0 + g);
        let c1p = (a1p * a1p + b1 * b1).sqrt();
        let c2p = (a2p * a2p + b2 * b2).sqrt();
        let avg_cp = (c1p + c2p) / 2.0;
        let hp = |x: f64, y: f64| {
            let mut a = x.atan2(y).to_degrees();
            if a < 0.0 {
                a += 360.0;
            }
            a
        };
        let h1p = if c1p == 0.0 { 0.0 } else { hp(b1, a1p) };
        let h2p = if c2p == 0.0 { 0.0 } else { hp(b2, a2p) };
        let dlp = l2 - l1;
        let dcp = c2p - c1p;
        let dhp = if c1p * c2p == 0.0 {
            0.0
        } else if (h2p - h1p).abs() <= 180.0 {
            h2p - h1p
        } else if h2p - h1p > 180.0 {
            h2p - h1p - 360.0
        } else {
            h2p - h1p + 360.0
        };
        let dhp = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
        let avg_hp = if c1p * c2p == 0.0 {
            h1p + h2p
        } else if (h1p - h2p).abs() <= 180.0 {
            (h1p + h2p) / 2.0
        } else if h1p + h2p < 360.0 {
            (h1p + h2p + 360.0) / 2.0
        } else {
            (h1p + h2p - 360.0) / 2.0
        };
        let t = 1.0 - 0.17 * (avg_hp - 30.0).to_radians().cos()
            + 0.24 * (2.0 * avg_hp).to_radians().cos()
            + 0.32 * (3.0 * avg_hp + 6.0).to_radians().cos()
            - 0.20 * (4.0 * avg_hp - 63.0).to_radians().cos();
        let d_ro = 30.0 * (-(((avg_hp - 275.0) / 25.0).powi(2))).exp();
        let rc = 2.0 * (avg_cp.powi(7) / (avg_cp.powi(7) + 25f64.powi(7))).sqrt();
        let sl = 1.0 + (0.015 * (avg_lp - 50.0).powi(2)) / (20.0 + (avg_lp - 50.0).powi(2)).sqrt();
        let sc = 1.0 + 0.045 * avg_cp;
        let sh = 1.0 + 0.015 * avg_cp * t;
        let rt = -(2.0 * d_ro).to_radians().sin() * rc;
        ((dlp / sl).powi(2)
            + (dcp / sc).powi(2)
            + (dhp / sh).powi(2)
            + rt * (dcp / sc) * (dhp / sh))
            .sqrt()
    }

    /// Machado et al. 2009 CVD simulation (severity 1.0), applied in linear RGB.
    fn simulate_cvd(c: Rgb, m: [[f64; 3]; 3]) -> Rgb {
        let lin = [
            srgb_to_lin(c.r as f64),
            srgb_to_lin(c.g as f64),
            srgb_to_lin(c.b as f64),
        ];
        let out: Vec<u8> = m
            .iter()
            .map(|row| {
                lin_to_byte(row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]).round() as u8
            })
            .collect();
        Rgb::new(out[0], out[1], out[2])
    }

    const CVD_MATRICES: [[[f64; 3]; 3]; 3] = [
        // Protanopia
        [
            [0.152286, 1.052583, -0.204868],
            [0.114503, 0.786281, 0.099216],
            [-0.003882, -0.048116, 1.051998],
        ],
        // Deuteranopia
        [
            [0.367322, 0.860646, -0.227968],
            [0.280085, 0.672501, 0.047413],
            [-0.011820, 0.042940, 0.968881],
        ],
        // Tritanopia
        [
            [1.255528, -0.076749, -0.178779],
            [-0.078411, 0.930809, 0.147602],
            [0.004733, 0.691367, 0.303900],
        ],
    ];

    const CONTENT: [WordCategory; 6] = [
        WordCategory::Particle,
        WordCategory::Noun,
        WordCategory::Verb,
        WordCategory::Adjective,
        WordCategory::Adverb,
        WordCategory::Expression,
    ];
    const ALL_CATEGORIES: [WordCategory; 9] = [
        WordCategory::Particle,
        WordCategory::Noun,
        WordCategory::Verb,
        WordCategory::Adjective,
        WordCategory::Adverb,
        WordCategory::Expression,
        WordCategory::Auxiliary,
        WordCategory::Other,
        WordCategory::Unknown,
    ];

    /// Returns each vision model's view of a colour: index 0 is normal vision,
    /// 1..=3 are the three dichromacies.
    fn under_all_vision(c: Rgb) -> Vec<Rgb> {
        let mut v = vec![c];
        v.extend(CVD_MATRICES.iter().map(|m| simulate_cvd(c, *m)));
        v
    }

    #[test]
    fn category_palette_is_cvd_safe() {
        // The six grammar-bearing categories must stay perceptually distinct from
        // one another for every reader, including under the three dichromacies.
        // dE2000 >= 10 is a clearly-noticeable difference; the optimized palette
        // sits at ~10.5, so a floor of 10 catches a real regression without
        // tripping on rounding.
        let mut worst = f64::INFINITY;
        let mut worst_pair = (WordCategory::Other, WordCategory::Other);
        for (i, &cat_a) in CONTENT.iter().enumerate() {
            for &cat_b in CONTENT.iter().skip(i + 1) {
                let a = under_all_vision(category_rgb(cat_a));
                let b = under_all_vision(category_rgb(cat_b));
                for (va, vb) in a.iter().zip(b.iter()) {
                    let d = ciede2000(rgb_to_lab(*va), rgb_to_lab(*vb));
                    if d < worst {
                        worst = d;
                        worst_pair = (cat_a, cat_b);
                    }
                }
            }
        }
        assert!(
            worst >= 10.0,
            "content category pair {worst_pair:?} collapses to dE2000 {worst:.1} \
             under some vision model (floor 10.0); rerun the palette optimizer"
        );
    }

    #[test]
    fn category_accent_bars_meet_panel_contrast() {
        // Each category colour is also drawn as a solid accent bar on the dark
        // popup panel; WCAG requires >= 3:1 for such non-text UI components.
        for cat in ALL_CATEGORIES {
            let ratio = contrast(category_rgb(cat), COLOR_PANEL_BG);
            assert!(
                ratio >= 3.0,
                "{cat:?} accent bar contrast {ratio:.2}:1 on the panel is below 3:1"
            );
        }
    }

    #[test]
    fn popup_body_text_meets_aa_contrast() {
        // Guards the primary accessibility anchor from PRODUCT.md: popup body
        // text stays readable on the panel (WCAG AA body >= 4.5:1).
        for (label, color) in [
            ("word", COLOR_WORD_TEXT),
            ("furigana", COLOR_FURIGANA),
            ("note", COLOR_NOTE_TEXT),
            ("gloss", COLOR_GLOSS_TEXT),
        ] {
            let ratio = contrast(color, COLOR_PANEL_BG);
            assert!(
                ratio >= 4.5,
                "{label} text contrast {ratio:.2}:1 on the panel is below AA 4.5:1"
            );
        }
    }
}
