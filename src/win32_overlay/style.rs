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
pub(super) fn category_rgb(category: WordCategory) -> Rgb {
    match category {
        WordCategory::Particle => Rgb::new(235, 195, 50), // amber
        WordCategory::Noun => Rgb::new(70, 150, 245),     // blue
        WordCategory::Verb => Rgb::new(90, 200, 120),     // green
        WordCategory::Adjective => Rgb::new(240, 150, 60), // orange
        WordCategory::Adverb => Rgb::new(190, 130, 235),  // purple
        WordCategory::Expression => Rgb::new(80, 200, 200), // teal
        WordCategory::Auxiliary => Rgb::new(150, 150, 165), // grey
        WordCategory::Other => Rgb::new(170, 170, 180),   // light grey
        WordCategory::Unknown => Rgb::new(120, 120, 135), // faint grey
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
