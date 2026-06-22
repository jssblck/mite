//! Per-pixel-alpha layered overlay window.
//!
//! Renders into a 32-bit BGRA DIB and presents it with `UpdateLayeredWindow`,
//! so decoration can be genuinely translucent (colour-key transparency is only
//! binary). Recognized words get a part-of-speech-coloured underline (unless
//! `overlay.word_underlines` is off) and, when `overlay.furigana` is enabled,
//! furigana drawn above them; the hovered word is tinted and the definition
//! popup is an opaque, rounded, furigana-topped panel with a category pill and a
//! tail pointing at the word. The window stays click-through
//! (`WS_EX_TRANSPARENT`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, FW_NORMAL, FW_SEMIBOLD, FillRect,
    GetStockObject, GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, HOLLOW_BRUSH, PS_SOLID,
    Polyline, Rectangle, RoundRect, SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor,
    TRANSPARENT, TextOutW,
};
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

use crate::geometry::{Rect, ScreenRect};
use crate::hover::{FuriSegment, Highlight, PopupContent, note_lead_len, strip_pos_tag};
use crate::hud::{LatencyHud, PassCounts, PassExtras, PassTimings, Stage};

mod platform;
mod style;

use platform::{
    create_aa_font, create_canvas, create_font, create_mono_font, create_overlay_window,
    present_layered, pump_messages, u32_to_i32,
};
use style::{
    COLOR_FURIGANA, COLOR_GLOSS_TEXT, COLOR_HUD_BG, COLOR_HUD_GRAPH_BG, COLOR_HUD_GRIDLINE,
    COLOR_HUD_LEGEND_HEADER, COLOR_HUD_STATUS, COLOR_HUD_TITLE, COLOR_NOTE_LEAD, COLOR_NOTE_TEXT,
    COLOR_PANEL_BG, COLOR_PANEL_BORDER, COLOR_SEPARATOR, COLOR_WHITE, COLOR_WORD_TEXT, Rgb,
    category_rgb, hud_stage_rgb,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayEvent {
    Hotkey(i32),
}

const CLASS_NAME: &str = "MiteOverlayWindow";
/// Title of the (invisible, tool) overlay window.
const WINDOW_TITLE: &str = "Mite OCR Overlay";

/// How long the blocking [`Win32Overlay::pump_for`] sleeps between message
/// pumps (~60 Hz, matching the watch UI loop).
const PUMP_POLL_INTERVAL: Duration = Duration::from_millis(16);

// Fonts. Negative heights request a character (em) height rather than a cell
// height; the popup uses a proportional UI face and the HUD a fixed-pitch face
// so its numeric columns line up.
const FONT_FACE_UI: &str = "Yu Gothic UI";
const FONT_FACE_MONO: &str = "Consolas";
const FONT_HEIGHT_WORD: i32 = -26;
const FONT_HEIGHT_FURIGANA: i32 = -15;
const FONT_HEIGHT_GLOSS: i32 = -18;
const FONT_HEIGHT_PILL: i32 = -13;
const FONT_HEIGHT_HUD: i32 = -16;

// Overlay word decoration: a category-coloured underline under each word, and
// the always-on furigana drawn above it.
/// Underline thickness as a fraction of the word-box height, clamped to a pixel
/// range so it stays a hairline on small text and never a slab on large text.
const UNDERLINE_RATIO: f32 = 0.06;
const UNDERLINE_MIN_PX: f32 = 2.0;
const UNDERLINE_MAX_PX: f32 = 4.0;
/// Extra thickness added to the hovered word's underline.
const UNDERLINE_HOVER_BONUS_PX: f32 = 2.0;
/// Underline alpha (out of 255) for known words, the hovered word, and unknown
/// (unresolved) words, which stay faint so they read as "not looked up".
const ALPHA_UNDERLINE: u32 = 190;
const ALPHA_UNDERLINE_HOVER: u32 = 255;
const ALPHA_UNDERLINE_UNKNOWN: u32 = 70;
/// Translucent fill tinting the hovered word's box.
const ALPHA_SELECTION: u32 = 44;
/// Fully opaque alpha (the panel/HUD backgrounds force this).
const ALPHA_OPAQUE: u8 = 255;

/// Overlay furigana height as a fraction of the word-box height, clamped to a
/// readable pixel range.
const OVERLAY_FURIGANA_RATIO: f32 = 0.36;
const OVERLAY_FURIGANA_MIN_PX: i32 = 10;
const OVERLAY_FURIGANA_MAX_PX: i32 = 44;
/// Gap between the top of the word box and the bottom of its furigana.
const OVERLAY_FURIGANA_GAP_PX: i32 = 2;

// Popup layout.
const POPUP_PADDING: i32 = 12;
const POPUP_MAX_WIDTH: i32 = 600;
/// Target wrap width for gloss and note body text before the panel is allowed
/// to grow; keeps definitions in a comfortable reading column.
const POPUP_WRAP_TARGET: i32 = 300;
const POPUP_CORNER_RADIUS: i32 = 12;
const LINE_GAP: i32 = 3;
/// Gap between the furigana row and the base word row in the heading.
const RUBY_GAP: i32 = 1;
/// Gaps around the category pill (heading -> pill, pill -> glosses).
const PILL_GAP_ABOVE: i32 = 8;
const PILL_GAP_BELOW: i32 = 9;
/// Inner padding of the pill badge, and per-character tracking of its label.
const PILL_PAD_X: i32 = 8;
const PILL_PAD_Y: i32 = 3;
const PILL_TRACKING: i32 = 1;
/// Spacing around the note's separator rule and the rule's own thickness.
const NOTE_GAP_ABOVE: i32 = 9;
const NOTE_GAP_BELOW: i32 = 8;
const SEPARATOR_H: i32 = 1;
/// The downward/upward tail connecting the popup to the hovered word.
const TAIL_HALF_W: i32 = 8;
const TAIL_H: i32 = 8;

// Latency HUD (top-left).
const HUD_WINDOW: Duration = Duration::from_secs(30);
const HUD_MARGIN: i32 = 14;
const HUD_PAD: i32 = 10;
const HUD_W: i32 = 470;
const HUD_GRAPH_H: i32 = 150;
const HUD_ROW_H: i32 = 18;
/// Vertical gap between stacked HUD sections (text block ↔ graph, legend ↔ footer).
const HUD_SECTION_GAP: i32 = 6;
/// Vertical gap between the graph and the legend header.
const HUD_GRAPH_LEGEND_GAP: i32 = 8;
/// Left indent of HUD legend text past its colour swatch.
const HUD_LEGEND_TEXT_INDENT: i32 = 18;
/// HUD legend colour swatch size/placement within a row.
const HUD_SWATCH_W: i32 = 12;
const HUD_SWATCH_H: i32 = 10;
const HUD_SWATCH_TOP_OFFSET: i32 = 4;
/// Fractions of the graph height at which faint horizontal gridlines are drawn.
const HUD_GRIDLINE_FRACTIONS: [f32; 3] = [0.25, 0.5, 0.75];
/// Pen thickness for the (emphasised) total series vs. the per-stage series.
const HUD_TOTAL_THICKNESS: i32 = 2;
const HUD_STAGE_THICKNESS: i32 = 1;

/// Pen width for thin 1px borders/outlines.
const HAIRLINE_PEN_WIDTH: i32 = 1;

/// A definition popup for the hovered word. `word_rect` is frame-local; the
/// panel is placed above the word (flipping below when there is no room) with a
/// tail pointing at it, and clamped to the overlay bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub word_rect: Rect,
    pub content: PopupContent,
}

/// An off-screen 32-bit BGRA surface backing the layered window.
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
    width: i32,
    height: i32,
}

/// The GDI fonts used by the popup, plus a fixed-pitch font for the latency HUD
/// (so its p50/p95/p99 columns line up).
#[derive(Debug, Clone, Copy)]
struct Fonts {
    word: HFONT,
    furi: HFONT,
    gloss: HFONT,
    /// Semibold gloss-sized face for the bold lead word of an inflection note.
    note_lead: HFONT,
    pill: HFONT,
    hud: HFONT,
}

#[derive(Debug)]
pub struct Win32Overlay {
    hwnd: HWND,
    fonts: Fonts,
    /// Grayscale-AA furigana fonts for the always-on overlay, cached by pixel
    /// height (word boxes vary in size across the frame, so the ruby font does
    /// too). Built lazily and freed on drop.
    furi_fonts: HashMap<i32, HFONT>,
    canvas: Option<Canvas>,
    screen_rect: ScreenRect,
    highlights: Vec<Highlight>,
    hovered: Option<usize>,
    popup: Option<Popup>,
    /// Frame-local rect of the drawn popup panel (for sticky hit-testing).
    popup_panel: Option<Rect>,
    /// Rolling per-stage latency samples. Always collected (so headless metrics
    /// dumps work); only drawn when `hud_visible`.
    hud: LatencyHud,
    /// Whether to draw the latency HUD overlay (the `--hud` flag).
    hud_visible: bool,
    /// Whether to draw always-on furigana above each word (the
    /// `overlay.furigana` config option). Off by default; underlines and the
    /// hover popup are unaffected.
    furigana_visible: bool,
    /// Whether to draw the per-word category underlines and hover tint (the
    /// `overlay.word_underlines` config option). When false the word layer is
    /// transparent, but highlights are still stored so hover hit-testing and the
    /// popup keep working.
    underlines_visible: bool,
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl Win32Overlay {
    pub fn new() -> Result<Self> {
        let hwnd = create_overlay_window()?;
        Ok(Self {
            hwnd,
            fonts: Fonts {
                word: create_font(FONT_HEIGHT_WORD, FW_SEMIBOLD.0 as i32),
                furi: create_font(FONT_HEIGHT_FURIGANA, FW_NORMAL.0 as i32),
                gloss: create_font(FONT_HEIGHT_GLOSS, FW_NORMAL.0 as i32),
                note_lead: create_font(FONT_HEIGHT_GLOSS, FW_SEMIBOLD.0 as i32),
                pill: create_font(FONT_HEIGHT_PILL, FW_SEMIBOLD.0 as i32),
                hud: create_mono_font(FONT_HEIGHT_HUD),
            },
            furi_fonts: HashMap::new(),
            canvas: None,
            screen_rect: ScreenRect::default(),
            highlights: Vec::new(),
            hovered: None,
            popup: None,
            popup_panel: None,
            hud: LatencyHud::new(HUD_WINDOW),
            hud_visible: false,
            furigana_visible: false,
            underlines_visible: true,
        })
    }

    /// Show the top-left latency HUD overlay (timing is collected regardless).
    pub fn enable_hud(&mut self) {
        self.hud_visible = true;
    }

    /// Enable or disable the always-on furigana drawn above each word. Underlines
    /// and the hover popup (which has its own furigana) are unaffected.
    pub fn set_furigana_visible(&mut self, enabled: bool) {
        self.furigana_visible = enabled;
    }

    /// Enable or disable the per-word category underlines and hover tint. When
    /// disabled the word layer is transparent, but the popup still appears on
    /// hover (hit-testing uses word geometry, not the drawn pixels).
    pub fn set_underlines_visible(&mut self, enabled: bool) {
        self.underlines_visible = enabled;
    }

    /// Append one pass's timings + content counts + capture extras. Always
    /// recorded; surfaces on the HUD (if visible) and in headless metrics dumps
    /// via [`Self::hud`].
    pub fn record_pass(&mut self, timings: PassTimings, counts: PassCounts, extras: PassExtras) {
        self.hud.record(timings, counts, extras);
    }

    /// Read-only access to the rolling timing window, for metrics reporting.
    pub fn hud(&self) -> &LatencyHud {
        &self.hud
    }

    /// Frame-local rect of the current popup panel, if shown.
    pub fn popup_panel(&self) -> Option<Rect> {
        self.popup_panel
    }

    /// A copy of the current overlay surface: `(width, height, premultiplied
    /// BGRA bytes)`, for compositing the overlay over a frame offline (e.g. the
    /// headless overlay preview).
    pub fn overlay_surface(&self) -> Option<(i32, i32, Vec<u8>)> {
        let canvas = self.canvas.as_ref()?;
        let len = (canvas.width as usize) * (canvas.height as usize) * 4;
        let bytes = unsafe { std::slice::from_raw_parts(canvas.bits, len) }.to_vec();
        Some((canvas.width, canvas.height, bytes))
    }

    /// Reposition over a captured region and replace the word highlights.
    pub fn present_snapshot(&mut self, screen_rect: ScreenRect, highlights: &[Highlight]) {
        self.screen_rect = screen_rect;
        self.highlights.clear();
        self.highlights.extend_from_slice(highlights);
        self.hovered = None;
        self.popup = None;
        self.render();
    }

    /// Update the hover emphasis and popup, re-rendering only on change.
    pub fn set_interaction(&mut self, hovered: Option<usize>, popup: Option<Popup>) {
        if self.hovered == hovered && self.popup == popup {
            return;
        }
        self.hovered = hovered;
        self.popup = popup;
        self.render();
    }

    /// Clear all overlay content (fully transparent).
    pub fn clear(&mut self) {
        self.highlights.clear();
        self.hovered = None;
        self.popup = None;
        self.render();
    }

    pub fn pump(&self) -> Vec<OverlayEvent> {
        pump_messages()
    }

    pub fn pump_for(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let _ = pump_messages();
            std::thread::sleep(PUMP_POLL_INTERVAL);
        }
    }

    /// Ensure a grayscale-AA furigana font of the given pixel height exists in
    /// the cache. Heights are bucketed to whole pixels, so the map stays small
    /// (a handful of distinct word sizes per scene).
    fn ensure_furi_font(&mut self, height: i32) {
        self.furi_fonts
            .entry(height)
            .or_insert_with(|| create_aa_font(-height, FW_NORMAL.0 as i32));
    }

    fn render(&mut self) {
        let width = u32_to_i32(self.screen_rect.size.width).max(1);
        let height = u32_to_i32(self.screen_rect.size.height).max(1);

        if self
            .canvas
            .as_ref()
            .map(|canvas| (canvas.width, canvas.height))
            != Some((width, height))
        {
            self.destroy_canvas();
            match create_canvas(width, height) {
                Some(canvas) => self.canvas = Some(canvas),
                None => return,
            }
        }
        // Build any furigana fonts this frame needs before borrowing the canvas
        // (the draw pass below only reads the cache). Skipped entirely when
        // overlay furigana is disabled.
        if self.furigana_visible {
            let needed_heights: Vec<i32> = self
                .highlights
                .iter()
                .filter(|h| h.ruby.iter().any(|seg| seg.furigana.is_some()))
                .map(|h| overlay_furigana_height(h.rect.height))
                .collect();
            for height in needed_heights {
                self.ensure_furi_font(height);
            }
        }

        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };

        // Clear to fully transparent.
        unsafe {
            std::ptr::write_bytes(canvas.bits, 0, (width as usize) * (height as usize) * 4);
        }

        // Furigana first, while the surface above each word is still cleared to
        // black: the grayscale glyphs' alpha is recovered from that black base.
        if self.furigana_visible {
            for highlight in &self.highlights {
                if !highlight.ruby.iter().any(|seg| seg.furigana.is_some()) {
                    continue;
                }
                let furi_h = overlay_furigana_height(highlight.rect.height);
                if let Some(&font) = self.furi_fonts.get(&furi_h) {
                    unsafe { draw_overlay_furigana(canvas, highlight.rect, &highlight.ruby, font) };
                }
            }
        }

        // Then the category underlines and the hovered word's selection tint.
        if self.underlines_visible {
            for (index, highlight) in self.highlights.iter().enumerate() {
                let hovered = self.hovered == Some(index);
                draw_word_underline(canvas.bits, width, height, highlight, hovered);
            }
        }

        let mut panel_rect = None;
        if let Some(popup) = &self.popup {
            panel_rect = Some(unsafe { draw_popup(canvas, width, height, popup, &self.fonts) });
        }

        // HUD draws last so it stays readable over highlights and the popup.
        if self.hud_visible {
            unsafe { draw_hud(canvas, width, height, &self.hud, self.fonts.hud) };
        }

        present_layered(self.hwnd, self.screen_rect, canvas);
        self.popup_panel = panel_rect;
    }

    fn destroy_canvas(&mut self) {
        if let Some(canvas) = self.canvas.take() {
            unsafe {
                let _ = DeleteDC(canvas.dc);
                let _ = DeleteObject(HGDIOBJ(canvas.bitmap.0));
            }
        }
    }
}

impl Drop for Win32Overlay {
    fn drop(&mut self) {
        self.destroy_canvas();
        unsafe {
            let fixed = [
                self.fonts.word,
                self.fonts.furi,
                self.fonts.gloss,
                self.fonts.note_lead,
                self.fonts.pill,
                self.fonts.hud,
            ];
            for font in fixed.into_iter().chain(self.furi_fonts.values().copied()) {
                if !font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Pixel height of the overlay furigana for a word box of the given height.
fn overlay_furigana_height(box_h: f32) -> i32 {
    (box_h * OVERLAY_FURIGANA_RATIO).round().clamp(
        OVERLAY_FURIGANA_MIN_PX as f32,
        OVERLAY_FURIGANA_MAX_PX as f32,
    ) as i32
}

/// Fill a half-open rect in the BGRA buffer with a premultiplied translucent
/// colour, alpha-blending over whatever is already there (so a selection tint
/// composites correctly with an underline beneath it).
fn fill_premultiplied(bits: *mut u8, width: i32, height: i32, rect: Rect, color: Rgb, alpha: u32) {
    let alpha = alpha.min(255);
    let x0 = (rect.x.round() as i32).clamp(0, width);
    let y0 = (rect.y.round() as i32).clamp(0, height);
    let x1 = (rect.right().round() as i32).clamp(0, width);
    let y1 = (rect.bottom().round() as i32).clamp(0, height);
    let sr = color.r as u32 * alpha / 255;
    let sg = color.g as u32 * alpha / 255;
    let sb = color.b as u32 * alpha / 255;
    let inv = 255 - alpha;
    for y in y0..y1 {
        let row = (y as isize) * (width as isize) * 4;
        for x in x0..x1 {
            let offset = row + (x as isize) * 4;
            unsafe {
                let db = *bits.offset(offset) as u32;
                let dg = *bits.offset(offset + 1) as u32;
                let dr = *bits.offset(offset + 2) as u32;
                let da = *bits.offset(offset + 3) as u32;
                *bits.offset(offset) = (sb + db * inv / 255) as u8;
                *bits.offset(offset + 1) = (sg + dg * inv / 255) as u8;
                *bits.offset(offset + 2) = (sr + dr * inv / 255) as u8;
                *bits.offset(offset + 3) = (alpha + da * inv / 255) as u8;
            }
        }
    }
}

/// Draw a word's category underline (and, when hovered, a translucent selection
/// tint over the box) into the BGRA buffer. Unresolved words get a fainter line
/// so they read as "not looked up".
fn draw_word_underline(
    bits: *mut u8,
    width: i32,
    height: i32,
    highlight: &Highlight,
    hovered: bool,
) {
    let color = category_rgb(highlight.category);
    if hovered {
        fill_premultiplied(bits, width, height, highlight.rect, color, ALPHA_SELECTION);
    }

    let mut thickness =
        (highlight.rect.height * UNDERLINE_RATIO).clamp(UNDERLINE_MIN_PX, UNDERLINE_MAX_PX);
    if hovered {
        thickness += UNDERLINE_HOVER_BONUS_PX;
    }
    let alpha = if !highlight.known {
        ALPHA_UNDERLINE_UNKNOWN
    } else if hovered {
        ALPHA_UNDERLINE_HOVER
    } else {
        ALPHA_UNDERLINE
    };
    let bar = Rect::new(
        highlight.rect.x,
        highlight.rect.bottom() - thickness,
        highlight.rect.width,
        thickness,
    );
    fill_premultiplied(bits, width, height, bar, color, alpha);
}

/// Draw a word's always-on furigana above its box. Each ruby segment is centred
/// over the horizontal span of the surface characters it covers (even spacing,
/// matching how the game lays out the glyphs underneath). Text is rendered with
/// grayscale AA and recoloured from its coverage so it carries correct per-pixel
/// alpha over the transparent overlay.
unsafe fn draw_overlay_furigana(canvas: &Canvas, rect: Rect, ruby: &[FuriSegment], font: HFONT) {
    let dc = canvas.dc;
    let total_chars: usize = ruby.iter().map(|seg| seg.text.chars().count()).sum();
    if total_chars == 0 {
        return;
    }
    let cell_w = rect.width / total_chars as f32;
    let (_, furi_h) = measure(dc, font, &utf16("あ"));
    let furi_top = (rect.y.round() as i32) - OVERLAY_FURIGANA_GAP_PX - furi_h;
    if furi_top < 0 {
        return;
    }

    unsafe {
        SetBkMode(dc, TRANSPARENT);
    }
    let mut char_offset = 0usize;
    for seg in ruby {
        let seg_chars = seg.text.chars().count();
        if let Some(reading) = &seg.furigana {
            let text = utf16(reading);
            let (furi_w, _) = measure(dc, font, &text);
            let seg_x0 = rect.x + char_offset as f32 * cell_w;
            let seg_w = seg_chars as f32 * cell_w;
            let fx = (seg_x0 + (seg_w - furi_w as f32) / 2.0).round() as i32;
            unsafe {
                blit_overlay_text(canvas, fx, furi_top, font, &text, COLOR_FURIGANA);
            }
        }
        char_offset += seg_chars;
    }
}

/// Draw `text` in `color` with correct per-pixel alpha onto the transparent
/// layered surface. GDI cannot write alpha, so we draw the glyphs in white onto
/// the (black, fully transparent) surface, then recover each pixel's coverage
/// from its grey level and re-emit it premultiplied in `color`. The font must be
/// grayscale-AA so the three channels agree (see `create_aa_font`).
unsafe fn blit_overlay_text(
    canvas: &Canvas,
    x: i32,
    y: i32,
    font: HFONT,
    text: &[u16],
    color: Rgb,
) {
    if text.is_empty() {
        return;
    }
    let dc = canvas.dc;
    let (frame_w, frame_h) = (canvas.width, canvas.height);
    let (w, h) = measure(dc, font, text);
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, COLOR_WHITE.to_colorref());
        let _ = TextOutW(dc, x, y, text);
        SelectObject(dc, old);
    }

    // Recover alpha from coverage over the glyphs' bounding box.
    let x0 = x.clamp(0, frame_w);
    let y0 = y.clamp(0, frame_h);
    let x1 = (x + w).clamp(0, frame_w);
    let y1 = (y + h).clamp(0, frame_h);
    for py in y0..y1 {
        let row = (py as isize) * (frame_w as isize) * 4;
        for px in x0..x1 {
            let offset = row + (px as isize) * 4;
            unsafe {
                // Grayscale AA: r == g == b == coverage. Untouched pixels are 0.
                let coverage = *canvas.bits.offset(offset + 1) as u32;
                if coverage == 0 {
                    continue;
                }
                *canvas.bits.offset(offset) = (color.b as u32 * coverage / 255) as u8;
                *canvas.bits.offset(offset + 1) = (color.g as u32 * coverage / 255) as u8;
                *canvas.bits.offset(offset + 2) = (color.r as u32 * coverage / 255) as u8;
                *canvas.bits.offset(offset + 3) = coverage as u8;
            }
        }
    }
}

/// One ruby segment laid out for drawing: base text + width, centred furigana +
/// width, and the cell width that hosts both. The cell is `max(base, furigana)`
/// so a reading wider than its kanji widens the heading instead of overhanging
/// the panel (and being clipped by the rounded-corner opacity mask).
struct RubySeg {
    text: Vec<u16>,
    width: i32,
    furi: Option<Vec<u16>>,
    furi_w: i32,
    cell: i32,
}

/// A laid-out category pill: its uppercase label plus padded box size.
struct PillLayout {
    text: Vec<u16>,
    width: i32,
    height: i32,
}

/// A wrapped body line: its UTF-16 text and measured size.
struct WrappedLine {
    text: Vec<u16>,
    width: i32,
    height: i32,
}

/// Returns the popup's `sticky_rect` in frame-local coordinates: its hover
/// region, extended across the tail gap to the word so the cursor can travel into
/// the popup without dropping the hover.
unsafe fn draw_popup(
    canvas: &Canvas,
    frame_w: i32,
    frame_h: i32,
    popup: &Popup,
    fonts: &Fonts,
) -> Rect {
    let dc = canvas.dc;
    let content = &popup.content;

    // --- Heading ruby (base in the word font, furigana in the small font) ---
    let mut segs = Vec::with_capacity(content.ruby.len());
    let mut word_w = 0;
    for segment in &content.ruby {
        let text = utf16(&segment.text);
        let (tw, _) = measure(dc, fonts.word, &text);
        let (furi, furi_w) = match &segment.furigana {
            Some(reading) => {
                let f = utf16(reading);
                let (fw, _) = measure(dc, fonts.furi, &f);
                (Some(f), fw)
            }
            None => (None, 0),
        };
        let cell = tw.max(furi_w);
        word_w += cell;
        segs.push(RubySeg {
            text,
            width: tw,
            furi,
            furi_w,
            cell,
        });
    }
    let has_furi = segs.iter().any(|seg| seg.furi.is_some());
    let (_, word_h) = measure(dc, fonts.word, &utf16("あ"));
    let (_, furi_h) = measure(dc, fonts.furi, &utf16("あ"));
    let heading_h = if has_furi {
        furi_h + RUBY_GAP + word_h
    } else {
        word_h
    };

    // --- Category pill ---
    let pill = content.pill.as_ref().map(|label| {
        let text = utf16(label);
        let (tw, th) = measure(dc, fonts.pill, &text);
        PillLayout {
            text: text.clone(),
            width: tw + PILL_TRACKING * text.len() as i32 + PILL_PAD_X * 2,
            height: th + PILL_PAD_Y * 2,
        }
    });

    // --- Body: glosses then optional note, wrapped to a reading column ---
    let wrap_w = (POPUP_MAX_WIDTH - POPUP_PADDING * 2).min(POPUP_WRAP_TARGET);
    let mut gloss_lines: Vec<WrappedLine> = Vec::new();
    for gloss in &content.glosses {
        gloss_lines.extend(wrap_text(dc, fonts.gloss, strip_pos_tag(gloss), wrap_w));
    }
    let note_lines: Vec<WrappedLine> = content
        .note
        .as_deref()
        .map(|note| wrap_text(dc, fonts.gloss, note, wrap_w))
        .unwrap_or_default();
    let lead_len = content.note.as_deref().map(note_lead_len).unwrap_or(0);

    // --- Panel size ---
    let body_w = gloss_lines
        .iter()
        .chain(&note_lines)
        .map(|line| line.width)
        .max()
        .unwrap_or(0);
    let pill_w = pill.as_ref().map_or(0, |p| p.width);
    let content_w = word_w.max(pill_w).max(body_w);
    // Floor the width so the panel can always host the tail between its rounded
    // corners; otherwise empty/tiny content makes the tail-x clamp below panic
    // (its lower bound would exceed its upper bound).
    let min_w = 2 * (POPUP_CORNER_RADIUS + TAIL_HALF_W);
    let panel_w = (content_w + POPUP_PADDING * 2).clamp(min_w, POPUP_MAX_WIDTH);

    let gap_before_gloss = if pill.is_some() {
        PILL_GAP_BELOW
    } else {
        PILL_GAP_ABOVE
    };
    let mut panel_h = POPUP_PADDING + heading_h;
    if let Some(p) = &pill {
        panel_h += PILL_GAP_ABOVE + p.height;
    }
    if !gloss_lines.is_empty() {
        panel_h += gap_before_gloss;
        for line in &gloss_lines {
            panel_h += line.height + LINE_GAP;
        }
    }
    if !note_lines.is_empty() {
        panel_h += NOTE_GAP_ABOVE + SEPARATOR_H + NOTE_GAP_BELOW;
        for (i, line) in note_lines.iter().enumerate() {
            panel_h += line.height;
            if i + 1 < note_lines.len() {
                panel_h += LINE_GAP;
            }
        }
    }
    panel_h += POPUP_PADDING;

    // --- Placement: centred over the word, preferring above with a tail down ---
    let word = popup.word_rect;
    let center_x = (word.x + word.width / 2.0).round() as i32;
    let left = (center_x - panel_w / 2).clamp(0, (frame_w - panel_w).max(0));
    let above_top = word.y.round() as i32 - TAIL_H - panel_h;
    let (top, tail_down) = if above_top >= 0 {
        (above_top, true)
    } else {
        (
            (word.bottom().round() as i32 + TAIL_H).min((frame_h - panel_h).max(0)),
            false,
        )
    };
    let top = top.max(0);

    let panel = RECT {
        left,
        top,
        right: left + panel_w,
        bottom: top + panel_h,
    };
    // Tail apex sits on the word edge; the base hugs the flat part of the panel
    // edge (kept off the rounded corners).
    let tail_x = center_x.clamp(
        left + POPUP_CORNER_RADIUS + TAIL_HALF_W,
        left + panel_w - POPUP_CORNER_RADIUS - TAIL_HALF_W,
    );

    let text_x = left + POPUP_PADDING;
    unsafe {
        // Panel background (the rounded corners are carved by the opacity mask).
        let bg = CreateSolidBrush(COLOR_PANEL_BG.to_colorref());
        FillRect(dc, &panel, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        SetBkMode(dc, TRANSPARENT);

        // Heading: each base segment, its furigana centred above.
        let furi_y = top + POPUP_PADDING;
        let word_y = furi_y + if has_furi { furi_h + RUBY_GAP } else { 0 };
        let mut x = text_x;
        for seg in &segs {
            // Base and furigana are each centred in the segment's cell, so a wide
            // reading sits symmetrically over its kanji without overhang.
            let old_font = SelectObject(dc, HGDIOBJ(fonts.word.0));
            SetTextColor(dc, COLOR_WORD_TEXT.to_colorref());
            if !seg.text.is_empty() {
                let _ = TextOutW(dc, x + (seg.cell - seg.width) / 2, word_y, &seg.text);
            }
            SelectObject(dc, old_font);

            if let Some(furi) = &seg.furi {
                let fx = x + (seg.cell - seg.furi_w) / 2;
                let old_font = SelectObject(dc, HGDIOBJ(fonts.furi.0));
                SetTextColor(dc, COLOR_FURIGANA.to_colorref());
                let _ = TextOutW(dc, fx, furi_y, furi);
                SelectObject(dc, old_font);
            }
            x += seg.cell;
        }

        let mut y = word_y + word_h;

        // Category pill.
        if let Some(p) = &pill {
            y += PILL_GAP_ABOVE;
            draw_pill(
                dc,
                text_x,
                y,
                p,
                category_rgb(content.category),
                &fonts.pill,
            );
            y += p.height;
        }

        // Gloss lines.
        if !gloss_lines.is_empty() {
            y += gap_before_gloss;
            let old_font = SelectObject(dc, HGDIOBJ(fonts.gloss.0));
            SetTextColor(dc, COLOR_GLOSS_TEXT.to_colorref());
            for line in &gloss_lines {
                if !line.text.is_empty() {
                    let _ = TextOutW(dc, text_x, y, &line.text);
                }
                y += line.height + LINE_GAP;
            }
            SelectObject(dc, old_font);
        }

        // Inflection note, under a hairline rule, with a bold lead word.
        if !note_lines.is_empty() {
            y += NOTE_GAP_ABOVE;
            let rule = RECT {
                left: text_x,
                top: y,
                right: left + panel_w - POPUP_PADDING,
                bottom: y + SEPARATOR_H,
            };
            let sep = CreateSolidBrush(COLOR_SEPARATOR.to_colorref());
            FillRect(dc, &rule, sep);
            let _ = DeleteObject(HGDIOBJ(sep.0));
            y += SEPARATOR_H + NOTE_GAP_BELOW;
            for (i, line) in note_lines.iter().enumerate() {
                let split = if i == 0 {
                    lead_len.min(line.text.len())
                } else {
                    0
                };
                let mut lx = text_x;
                if split > 0 {
                    let old_font = SelectObject(dc, HGDIOBJ(fonts.note_lead.0));
                    SetTextColor(dc, COLOR_NOTE_LEAD.to_colorref());
                    let _ = TextOutW(dc, lx, y, &line.text[..split]);
                    let (lw, _) = measure(dc, fonts.note_lead, &line.text[..split]);
                    SelectObject(dc, old_font);
                    lx += lw;
                }
                let old_font = SelectObject(dc, HGDIOBJ(fonts.gloss.0));
                SetTextColor(dc, COLOR_NOTE_TEXT.to_colorref());
                if split < line.text.len() {
                    let _ = TextOutW(dc, lx, y, &line.text[split..]);
                }
                SelectObject(dc, old_font);
                y += line.height + LINE_GAP;
            }
        }

        // Tail, then carve the rounded corners. The tail is filled premultiplied
        // (already opaque), so the panel-only opacity mask leaves it untouched.
        fill_tail(
            canvas.bits,
            frame_w,
            frame_h,
            tail_x,
            top,
            panel_h,
            tail_down,
        );
    }

    apply_rounded_opacity(canvas.bits, frame_w, frame_h, &panel, POPUP_CORNER_RADIUS);

    // Sticky region: the panel plus the tail gap back to the word.
    if tail_down {
        Rect::new(
            left as f32,
            top as f32,
            panel_w as f32,
            (panel_h + TAIL_H) as f32,
        )
    } else {
        Rect::new(
            left as f32,
            (top - TAIL_H) as f32,
            panel_w as f32,
            (panel_h + TAIL_H) as f32,
        )
    }
}

/// Draw the category pill: a colour-outlined, fully-rounded badge with its
/// tracked uppercase label in the same colour.
unsafe fn draw_pill(dc: HDC, x: i32, y: i32, pill: &PillLayout, color: Rgb, font: &HFONT) {
    unsafe {
        let pen = CreatePen(PS_SOLID, HAIRLINE_PEN_WIDTH, color.to_colorref());
        let old_pen = SelectObject(dc, HGDIOBJ(pen.0));
        let hollow = GetStockObject(HOLLOW_BRUSH);
        let old_brush = SelectObject(dc, hollow);
        // Ellipse axes == the box height gives semicircular (pill) ends.
        let _ = RoundRect(
            dc,
            x,
            y,
            x + pill.width,
            y + pill.height,
            pill.height,
            pill.height,
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));

        SetTextCharacterExtra(dc, PILL_TRACKING);
        let old_font = SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, color.to_colorref());
        let _ = TextOutW(dc, x + PILL_PAD_X, y + PILL_PAD_Y, &pill.text);
        SelectObject(dc, old_font);
        SetTextCharacterExtra(dc, 0);
    }
}

/// Draw the latency HUD (per-stage time-series graph + p50/p95/p99 readout) in
/// the top-left corner. Uses GDI for fills/lines/text, then forces the panel
/// opaque (GDI leaves the alpha channel at 0).
unsafe fn draw_hud(canvas: &Canvas, frame_w: i32, frame_h: i32, hud: &LatencyHud, font: HFONT) {
    if hud.is_empty() {
        return;
    }
    let dc = canvas.dc;
    let rows = Stage::ALL.len() as i32;
    let title_h = HUD_ROW_H;
    let status_h = HUD_ROW_H;
    let header_h = HUD_ROW_H;
    let panel_w = HUD_W;
    let panel_h = HUD_PAD * 2
        + title_h
        + status_h
        + HUD_SECTION_GAP
        + HUD_GRAPH_H
        + HUD_GRAPH_LEGEND_GAP
        + header_h
        + rows * HUD_ROW_H
        + HUD_SECTION_GAP
        + HUD_ROW_H;
    let left = HUD_MARGIN;
    let top = HUD_MARGIN;
    let panel = RECT {
        left,
        top,
        right: left + panel_w,
        bottom: top + panel_h,
    };

    unsafe {
        // Panel background + border.
        let bg = CreateSolidBrush(COLOR_HUD_BG.to_colorref());
        FillRect(dc, &panel, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let pen = CreatePen(
            PS_SOLID,
            HAIRLINE_PEN_WIDTH,
            COLOR_PANEL_BORDER.to_colorref(),
        );
        let old_pen = SelectObject(dc, HGDIOBJ(pen.0));
        let hollow = GetStockObject(HOLLOW_BRUSH);
        let old_brush = SelectObject(dc, hollow);
        let _ = Rectangle(dc, panel.left, panel.top, panel.right, panel.bottom);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));

        SetBkMode(dc, TRANSPARENT);
        let old_font = SelectObject(dc, HGDIOBJ(font.0));

        // Title with the window, sample count, and current y-axis scale.
        let max_ms = hud.max_ms();
        let title = format!(
            "latency ms   {}s   n={}   max={:.1}",
            hud.window().as_secs(),
            hud.len(),
            max_ms
        );
        SetTextColor(dc, COLOR_HUD_TITLE.to_colorref());
        let _ = TextOutW(dc, left + HUD_PAD, top + HUD_PAD, &utf16(&title));

        // Status line: latest-pass content counts.
        let counts = hud.counts();
        let status = format!(
            "lines {}    words {}    known {}",
            counts.lines, counts.words, counts.known
        );
        SetTextColor(dc, COLOR_HUD_STATUS.to_colorref());
        let _ = TextOutW(dc, left + HUD_PAD, top + HUD_PAD + title_h, &utf16(&status));

        // Graph area.
        let gx = left + HUD_PAD;
        let gy = top + HUD_PAD + title_h + status_h + HUD_SECTION_GAP;
        let gw = panel_w - HUD_PAD * 2;
        let gh = HUD_GRAPH_H;
        let graph = RECT {
            left: gx,
            top: gy,
            right: gx + gw,
            bottom: gy + gh,
        };
        let graph_bg = CreateSolidBrush(COLOR_HUD_GRAPH_BG.to_colorref());
        FillRect(dc, &graph, graph_bg);
        let _ = DeleteObject(HGDIOBJ(graph_bg.0));

        // Faint horizontal gridlines at 25/50/75%.
        let grid = CreateSolidBrush(COLOR_HUD_GRIDLINE.to_colorref());
        for frac in HUD_GRIDLINE_FRACTIONS {
            let yy = gy + (gh as f32 * frac) as i32;
            let line = RECT {
                left: gx,
                top: yy,
                right: gx + gw,
                bottom: yy + 1,
            };
            FillRect(dc, &line, grid);
        }
        let _ = DeleteObject(HGDIOBJ(grid.0));

        // Stage polylines, scaled to the window's max total. Total is drawn last
        // (on top) and thicker.
        let scale = max_ms.max(1.0);
        let denom = (hud.len().max(2) - 1) as f32;
        for stage in Stage::ALL.into_iter().rev() {
            let series = hud.series(stage);
            if series.len() < 2 {
                continue;
            }
            let thickness = if stage == Stage::Total {
                HUD_TOTAL_THICKNESS
            } else {
                HUD_STAGE_THICKNESS
            };
            let pen = CreatePen(PS_SOLID, thickness, hud_stage_rgb(stage).to_colorref());
            let old = SelectObject(dc, HGDIOBJ(pen.0));
            let points: Vec<POINT> = series
                .iter()
                .enumerate()
                .map(|(i, &value)| {
                    let x = gx + ((i as f32 / denom) * gw as f32).round() as i32;
                    let y = gy + gh - ((value / scale) * gh as f32).round() as i32;
                    POINT {
                        x: x.clamp(gx, gx + gw),
                        y: y.clamp(gy, gy + gh),
                    }
                })
                .collect();
            let _ = Polyline(dc, &points);
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }

        // Legend: column header, then one row per stage.
        let lx = left + HUD_PAD;
        let mut ly = gy + gh + HUD_GRAPH_LEGEND_GAP;
        let header = format!("{:<10}{:>7}{:>7}{:>7}", "", "p50", "p95", "p99");
        SetTextColor(dc, COLOR_HUD_LEGEND_HEADER.to_colorref());
        let _ = TextOutW(dc, lx + HUD_LEGEND_TEXT_INDENT, ly, &utf16(&header));
        ly += header_h;
        for stage in Stage::ALL {
            let color = hud_stage_rgb(stage);
            let swatch = RECT {
                left: lx,
                top: ly + HUD_SWATCH_TOP_OFFSET,
                right: lx + HUD_SWATCH_W,
                bottom: ly + HUD_SWATCH_TOP_OFFSET + HUD_SWATCH_H,
            };
            let brush = CreateSolidBrush(color.to_colorref());
            FillRect(dc, &swatch, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));

            let p = hud.percentiles(stage);
            let row = format!(
                "{:<10}{:>7.1}{:>7.1}{:>7.1}",
                stage.label(),
                p.p50,
                p.p95,
                p.p99
            );
            SetTextColor(dc, color.to_colorref());
            let _ = TextOutW(dc, lx + HUD_LEGEND_TEXT_INDENT, ly, &utf16(&row));
            ly += HUD_ROW_H;
        }

        // Capture-health footer: throughput, drop count, and frame staleness —
        // the metrics that surface the capture pipeline keeping (or not keeping)
        // up, which the per-stage latencies alone don't reveal.
        let age = hud.staging_age_percentiles();
        let footer = format!(
            "fps {:.1}    drop {:.1}    age {:.0}/{:.0}ms",
            hud.passes_per_sec(),
            hud.avg_frames_delivered(),
            age.p50,
            age.p95,
        );
        SetTextColor(dc, COLOR_HUD_STATUS.to_colorref());
        let _ = TextOutW(
            dc,
            lx + HUD_LEGEND_TEXT_INDENT,
            ly + HUD_SECTION_GAP,
            &utf16(&footer),
        );

        SelectObject(dc, old_font);
    }

    force_opaque(canvas.bits, frame_w, frame_h, &panel);
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Measure a UTF-16 string in the given font: (width, height) in pixels.
fn measure(dc: HDC, font: HFONT, text: &[u16]) -> (i32, i32) {
    let old = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
    let mut size = SIZE::default();
    let probe = if text.is_empty() {
        &[0x20u16][..]
    } else {
        text
    };
    let _ = unsafe { GetTextExtentPoint32W(dc, probe, &mut size) };
    unsafe { SelectObject(dc, old) };
    (size.cx, size.cy)
}

/// Greedily word-wrap `text` to lines no wider than `max_w` in `font`. Breaks at
/// spaces where possible; a single run longer than `max_w` (e.g. CJK with no
/// spaces) is broken between characters so it never overflows the panel.
fn wrap_text(dc: HDC, font: HFONT, text: &str, max_w: i32) -> Vec<WrappedLine> {
    let chars: Vec<char> = text.chars().collect();
    let (_, line_h) = measure(dc, font, &utf16("あ"));
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut last_space: Option<usize> = None;
    let mut i = 0usize;
    let push = |lines: &mut Vec<WrappedLine>, slice: &[char]| {
        let s: String = slice.iter().collect();
        let s = s.trim_end();
        let text = utf16(s);
        let (width, _) = measure(dc, font, &text);
        lines.push(WrappedLine {
            text,
            width,
            height: line_h,
        });
    };
    while i < chars.len() {
        let candidate = utf16(&chars[start..=i].iter().collect::<String>());
        let (w, _) = measure(dc, font, &candidate);
        if w > max_w && i > start {
            let break_at = match last_space {
                Some(s) if s > start => s,
                _ => i,
            };
            push(&mut lines, &chars[start..break_at]);
            start = if matches!(last_space, Some(s) if s == break_at) {
                break_at + 1 // consume the space we broke on
            } else {
                break_at
            };
            last_space = None;
            i = start;
            continue;
        }
        if chars[i] == ' ' {
            last_space = Some(i);
        }
        i += 1;
    }
    if start < chars.len() {
        push(&mut lines, &chars[start..]);
    }
    lines
}

/// Fill the popup's connector tail: an isosceles triangle whose base lies on the
/// panel edge at `tail_x` and whose apex points at the word. `tail_down` puts the
/// base on the panel bottom (apex below); otherwise the base is on the top (apex
/// above). Filled opaque in the panel colour, premultiplied.
fn fill_tail(
    bits: *mut u8,
    frame_w: i32,
    frame_h: i32,
    tail_x: i32,
    panel_top: i32,
    panel_h: i32,
    tail_down: bool,
) {
    let Rgb { r, g, b } = COLOR_PANEL_BG;
    let base_y = if tail_down {
        panel_top + panel_h
    } else {
        panel_top
    };
    for step in 0..TAIL_H {
        // Half-width shrinks linearly from the base to the apex.
        let half = (TAIL_HALF_W * (TAIL_H - step) / TAIL_H).max(0);
        let y = if tail_down {
            base_y + step
        } else {
            base_y - 1 - step
        };
        if y < 0 || y >= frame_h {
            continue;
        }
        let x0 = (tail_x - half).clamp(0, frame_w);
        let x1 = (tail_x + half).clamp(0, frame_w);
        let row = (y as isize) * (frame_w as isize) * 4;
        for x in x0..x1 {
            let offset = row + (x as isize) * 4;
            unsafe {
                *bits.offset(offset) = b;
                *bits.offset(offset + 1) = g;
                *bits.offset(offset + 2) = r;
                *bits.offset(offset + 3) = ALPHA_OPAQUE;
            }
        }
    }
}

/// Make the popup panel opaque with antialiased rounded corners. Interior pixels
/// get full alpha (GDI leaves the RGB correct); corner pixels get fractional
/// coverage from their distance to the corner arc, premultiplied so the edge is
/// a clean soft curve rather than a stair-step.
fn apply_rounded_opacity(bits: *mut u8, frame_w: i32, frame_h: i32, rect: &RECT, radius: i32) {
    let x0 = rect.left.clamp(0, frame_w);
    let y0 = rect.top.clamp(0, frame_h);
    let x1 = rect.right.clamp(0, frame_w);
    let y1 = rect.bottom.clamp(0, frame_h);
    let r = radius as f32;
    let (l, t, rt, bt) = (
        rect.left as f32,
        rect.top as f32,
        rect.right as f32,
        rect.bottom as f32,
    );
    for y in y0..y1 {
        let fy = y as f32 + 0.5;
        let dy = ((t + r) - fy).max(fy - (bt - r)).max(0.0);
        let row = (y as isize) * (frame_w as isize) * 4;
        for x in x0..x1 {
            let fx = x as f32 + 0.5;
            let dx = ((l + r) - fx).max(fx - (rt - r)).max(0.0);
            let coverage = if dx == 0.0 || dy == 0.0 {
                1.0
            } else {
                (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
            };
            let offset = row + (x as isize) * 4;
            unsafe {
                if coverage >= 1.0 {
                    *bits.offset(offset + 3) = ALPHA_OPAQUE;
                } else {
                    let scale = |c: u8| (c as f32 * coverage).round() as u8;
                    *bits.offset(offset) = scale(*bits.offset(offset));
                    *bits.offset(offset + 1) = scale(*bits.offset(offset + 1));
                    *bits.offset(offset + 2) = scale(*bits.offset(offset + 2));
                    *bits.offset(offset + 3) = (255.0 * coverage).round() as u8;
                }
            }
        }
    }
}

/// Force every pixel in `rect` to alpha 255 (premultiplied with a==255 leaves
/// the GDI-written RGB unchanged), making the HUD panel opaque.
fn force_opaque(bits: *mut u8, width: i32, height: i32, rect: &RECT) {
    let x0 = rect.left.clamp(0, width);
    let y0 = rect.top.clamp(0, height);
    let x1 = rect.right.clamp(0, width);
    let y1 = rect.bottom.clamp(0, height);
    for y in y0..y1 {
        let row = (y as isize) * (width as isize) * 4;
        for x in x0..x1 {
            unsafe {
                *bits.offset(row + (x as isize) * 4 + 3) = ALPHA_OPAQUE;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a `(b, g, r, a)` pixel from a BGRA buffer at `(x, y)`.
    fn px(buf: &[u8], w: i32, x: i32, y: i32) -> (u8, u8, u8, u8) {
        let o = ((y * w + x) * 4) as usize;
        (buf[o], buf[o + 1], buf[o + 2], buf[o + 3])
    }

    #[test]
    fn overlay_furigana_height_scales_and_clamps() {
        assert_eq!(overlay_furigana_height(50.0), 18); // 50 * 0.36
        assert_eq!(overlay_furigana_height(10.0), OVERLAY_FURIGANA_MIN_PX); // clamps up
        assert_eq!(overlay_furigana_height(400.0), OVERLAY_FURIGANA_MAX_PX); // clamps down
    }

    #[test]
    fn fill_premultiplied_blends_over_destination() {
        let mut buf = vec![0u8; 4 * 4 * 4];
        fill_premultiplied(
            buf.as_mut_ptr(),
            4,
            4,
            Rect::new(0.0, 0.0, 4.0, 4.0),
            Rgb {
                r: 200,
                g: 100,
                b: 50,
            },
            128,
        );
        // Over a transparent (zero) destination: just the premultiplied source.
        assert_eq!(px(&buf, 4, 1, 1), (25, 50, 100, 128));
    }

    #[test]
    fn rounded_opacity_opens_interior_and_carves_corners() {
        // A 16x16 panel pre-filled mid-grey with GDI's zero alpha.
        let (w, h) = (16, 16);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for chunk in buf.chunks_mut(4) {
            chunk[0] = 100;
            chunk[1] = 100;
            chunk[2] = 100;
        }
        let rect = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        apply_rounded_opacity(buf.as_mut_ptr(), w, h, &rect, 5);
        // Interior: fully opaque, RGB untouched.
        assert_eq!(px(&buf, w, 8, 8), (100, 100, 100, 255));
        // Straight top edge (between the corner arcs): also opaque.
        assert_eq!(px(&buf, w, 8, 0).3, 255);
        // Extreme corner: carved away (transparent, premultiplied to black).
        assert_eq!(px(&buf, w, 0, 0), (0, 0, 0, 0));
    }

    #[test]
    fn word_underline_sits_at_the_box_bottom() {
        // 20x20 transparent buffer; underline a full-height word box.
        let (w, h) = (20, 20);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let highlight = Highlight {
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            category: crate::hover::WordCategory::Noun,
            known: true,
            ruby: Vec::new(),
        };
        draw_word_underline(buf.as_mut_ptr(), w, h, &highlight, false);
        // Top of the box is untouched; the bottom rows carry the underline.
        assert_eq!(px(&buf, w, 10, 1).3, 0);
        assert_eq!(px(&buf, w, 10, 19).3, ALPHA_UNDERLINE as u8);
    }
}
