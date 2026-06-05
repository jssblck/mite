//! Per-pixel-alpha layered overlay window.
//!
//! Renders into a 32-bit BGRA DIB and presents it with `UpdateLayeredWindow`,
//! so highlights can be genuinely translucent (colour-key transparency is only
//! binary). Recognized words are drawn as translucent, part-of-speech-coloured
//! fills; the hovered word's definition popup is an opaque, furigana-topped
//! panel. The window stays click-through (`WS_EX_TRANSPARENT`).

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, Ellipse, FW_NORMAL, FW_SEMIBOLD, FillRect,
    GetStockObject, GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, HOLLOW_BRUSH, PS_SOLID,
    Polyline, Rectangle, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_TRANSPARENT,
};

use crate::geometry::{Rect, ScreenRect};
use crate::hover::{Highlight, PopupContent};
use crate::hud::{LatencyHud, PassCounts, PassExtras, PassTimings, Stage};

mod platform;
mod style;

use platform::{
    create_canvas, create_font, create_mono_font, create_overlay_window, present_layered,
    pump_messages, u32_to_i32,
};
use style::{
    COLOR_BUTTON_BG, COLOR_BUTTON_BORDER, COLOR_BUTTON_ICON, COLOR_FURIGANA, COLOR_GLOSS_TEXT,
    COLOR_HUD_BG, COLOR_HUD_GRAPH_BG, COLOR_HUD_GRIDLINE, COLOR_HUD_LEGEND_HEADER,
    COLOR_HUD_STATUS, COLOR_HUD_TITLE, COLOR_NOTE_TEXT, COLOR_PANEL_BG, COLOR_PANEL_BORDER,
    COLOR_WORD_TEXT, Rgb, category_rgb, hud_stage_rgb,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayEvent {
    Hotkey(i32),
    LeftButtonDown { x: i32, y: i32 },
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
const FONT_HEIGHT_HUD: i32 = -16;

// Highlight alphas (out of 255).
const ALPHA_UNKNOWN: u32 = 26;
const ALPHA_WORD: u32 = 64;
const ALPHA_HOVER: u32 = 130;
/// Fully opaque alpha (the panel/HUD backgrounds force this).
const ALPHA_OPAQUE: u8 = 255;

// Popup layout.
const POPUP_PADDING: i32 = 11;
const POPUP_ACCENT_W: i32 = 4;
const POPUP_MAX_WIDTH: i32 = 600;
const LINE_GAP: i32 = 3;
/// Vertical gap between the hovered word and the popup placed below it, small
/// enough that the cursor can cross into the popup without leaving the sticky
/// region.
const POPUP_ANCHOR_GAP: i32 = 2;
/// Gap between the furigana row and the base word row in the heading.
const RUBY_GAP: i32 = 1;

// Problem-report button (top-right of the popup).
const BUTTON_W: i32 = 26;
const BUTTON_H: i32 = 20;
const BUTTON_MARGIN: i32 = 8;

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

/// A definition popup near the hovered word. `anchor` is frame-local; the panel
/// is placed just below it and clamped to the overlay bounds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Popup {
    pub anchor_x: i32,
    pub anchor_y: i32,
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
    hud: HFONT,
}

#[derive(Debug)]
pub struct Win32Overlay {
    hwnd: HWND,
    fonts: Fonts,
    canvas: Option<Canvas>,
    screen_rect: ScreenRect,
    highlights: Vec<Highlight>,
    hovered: Option<usize>,
    popup: Option<Popup>,
    /// Frame-local rect of the drawn popup panel (for sticky hit-testing).
    popup_panel: Option<Rect>,
    /// Frame-local rect of the problem-report button (for click detection).
    screenshot_button: Option<Rect>,
    /// Whether the window currently passes clicks through (`WS_EX_TRANSPARENT`).
    click_through: bool,
    /// Rolling per-stage latency samples. Always collected (so headless metrics
    /// dumps work); only drawn when `hud_visible`.
    hud: LatencyHud,
    /// Whether to draw the latency HUD overlay (the `--hud` flag).
    hud_visible: bool,
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
                hud: create_mono_font(FONT_HEIGHT_HUD),
            },
            canvas: None,
            screen_rect: ScreenRect::default(),
            highlights: Vec::new(),
            hovered: None,
            popup: None,
            popup_panel: None,
            screenshot_button: None,
            click_through: true,
            hud: LatencyHud::new(HUD_WINDOW),
            hud_visible: false,
        })
    }

    /// Toggle click-through. The overlay is normally click-through so the game
    /// keeps all input; disable it briefly while the cursor is over an
    /// interactive element (the problem-report button) so the click lands on
    /// the overlay instead of passing through to the game.
    pub fn set_click_through(&mut self, enabled: bool) {
        if enabled == self.click_through {
            return;
        }
        self.click_through = enabled;
        let bit = WS_EX_TRANSPARENT.0 as isize;
        unsafe {
            let current = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let updated = if enabled {
                current | bit
            } else {
                current & !bit
            };
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, updated);
        }
    }

    /// Show the top-left latency HUD overlay (timing is collected regardless).
    pub fn enable_hud(&mut self) {
        self.hud_visible = true;
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

    /// Frame-local rect of the current problem-report button, if shown.
    pub fn screenshot_button(&self) -> Option<Rect> {
        self.screenshot_button
    }

    /// A copy of the current overlay surface: `(width, height, premultiplied
    /// BGRA bytes)`, for compositing into a debug capture.
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
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };

        // Clear to fully transparent.
        unsafe {
            std::ptr::write_bytes(canvas.bits, 0, (width as usize) * (height as usize) * 4);
        }

        for (index, highlight) in self.highlights.iter().enumerate() {
            let hovered = self.hovered == Some(index);
            fill_highlight(canvas.bits, width, height, highlight, hovered);
        }

        let mut panel_rect = None;
        let mut button_rect = None;
        if let Some(popup) = &self.popup {
            let (panel, button) = unsafe { draw_popup(canvas, width, height, popup, &self.fonts) };
            panel_rect = Some(panel);
            button_rect = Some(button);
        }

        // HUD draws last so it stays readable over highlights and the popup.
        if self.hud_visible {
            unsafe { draw_hud(canvas, width, height, &self.hud, self.fonts.hud) };
        }

        present_layered(self.hwnd, self.screen_rect, canvas);
        self.popup_panel = panel_rect;
        self.screenshot_button = button_rect;
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
            for font in [
                self.fonts.word,
                self.fonts.furi,
                self.fonts.gloss,
                self.fonts.hud,
            ] {
                if !font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Fill a word's rect into the BGRA buffer with a premultiplied translucent
/// colour (overwriting; word rects are disjoint).
fn fill_highlight(bits: *mut u8, width: i32, height: i32, highlight: &Highlight, hovered: bool) {
    let Rgb { r, g, b } = category_rgb(highlight.category);
    let alpha = if !highlight.known {
        ALPHA_UNKNOWN
    } else if hovered {
        ALPHA_HOVER
    } else {
        ALPHA_WORD
    };
    let pr = (r as u32 * alpha / 255) as u8;
    let pg = (g as u32 * alpha / 255) as u8;
    let pb = (b as u32 * alpha / 255) as u8;
    let pa = alpha as u8;

    let rect = highlight.rect;
    let x0 = (rect.x.floor() as i32).clamp(0, width);
    let y0 = (rect.y.floor() as i32).clamp(0, height);
    let x1 = (rect.right().ceil() as i32).clamp(0, width);
    let y1 = (rect.bottom().ceil() as i32).clamp(0, height);

    for y in y0..y1 {
        let row = (y as isize) * (width as isize) * 4;
        for x in x0..x1 {
            let offset = row + (x as isize) * 4;
            unsafe {
                *bits.offset(offset) = pb;
                *bits.offset(offset + 1) = pg;
                *bits.offset(offset + 2) = pr;
                *bits.offset(offset + 3) = pa;
            }
        }
    }
}

struct PopupLine {
    text: Vec<u16>,
    font: HFONT,
    color: COLORREF,
    width: i32,
    height: i32,
}

/// One ruby segment laid out for drawing: base text + width, and centred
/// furigana + width.
struct RubySeg {
    text: Vec<u16>,
    width: i32,
    furi: Option<Vec<u16>>,
    furi_w: i32,
}

/// Returns `(panel_rect, button_rect)` in frame-local coordinates.
unsafe fn draw_popup(
    canvas: &Canvas,
    frame_w: i32,
    frame_h: i32,
    popup: &Popup,
    fonts: &Fonts,
) -> (Rect, Rect) {
    let dc = canvas.dc;
    let content = &popup.content;

    // Heading: ruby segments (base in the word font, furigana in the small font).
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
        word_w += tw;
        segs.push(RubySeg {
            text,
            width: tw,
            furi,
            furi_w,
        });
    }
    let has_furi = segs.iter().any(|seg| seg.furi.is_some());
    let (_, word_h) = measure(dc, fonts.word, &utf16("あ"));
    let (_, furi_h) = measure(dc, fonts.furi, &utf16("あ"));

    // Body lines: inflection note, then glosses.
    let mut body: Vec<PopupLine> = Vec::new();
    if let Some(note) = &content.note {
        body.push(make_line(
            dc,
            note,
            fonts.gloss,
            COLOR_NOTE_TEXT.to_colorref(),
        ));
    }
    for gloss in &content.glosses {
        body.push(make_line(
            dc,
            gloss,
            fonts.gloss,
            COLOR_GLOSS_TEXT.to_colorref(),
        ));
    }

    let body_w = body.iter().map(|line| line.width).max().unwrap_or(0);
    let content_w = word_w.max(body_w);
    // Reserve a right-hand strip for the problem-report button.
    let panel_w = (content_w + POPUP_PADDING * 2 + POPUP_ACCENT_W + BUTTON_W + BUTTON_MARGIN)
        .min(POPUP_MAX_WIDTH);
    let heading_h = if has_furi {
        furi_h + RUBY_GAP + word_h
    } else {
        word_h
    };
    let body_h: i32 = body.iter().map(|line| line.height + LINE_GAP).sum();
    let panel_h = POPUP_PADDING * 2 + heading_h + body_h;

    // Place just below the anchor (small gap so the cursor can cross from the
    // word to the popup without leaving the sticky region), flipping/shifting to
    // stay on-screen.
    let mut left = popup.anchor_x;
    let mut top = popup.anchor_y + POPUP_ANCHOR_GAP;
    if left + panel_w > frame_w {
        left = (frame_w - panel_w).max(0);
    }
    if top + panel_h > frame_h {
        top = (popup.anchor_y - panel_h - POPUP_ANCHOR_GAP).max(0);
    }
    left = left.max(0);
    top = top.max(0);

    let panel = RECT {
        left,
        top,
        right: left + panel_w,
        bottom: top + panel_h,
    };
    let button = RECT {
        left: left + panel_w - BUTTON_W - BUTTON_MARGIN,
        top: top + BUTTON_MARGIN,
        right: left + panel_w - BUTTON_MARGIN,
        bottom: top + BUTTON_MARGIN + BUTTON_H,
    };

    unsafe {
        // Panel background.
        let bg = CreateSolidBrush(COLOR_PANEL_BG.to_colorref());
        FillRect(dc, &panel, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        // Left accent bar tinted by the word's category.
        let accent_brush = CreateSolidBrush(category_rgb(content.category).to_colorref());
        let accent = RECT {
            left,
            top,
            right: left + POPUP_ACCENT_W,
            bottom: top + panel_h,
        };
        FillRect(dc, &accent, accent_brush);
        let _ = DeleteObject(HGDIOBJ(accent_brush.0));

        // Border.
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
        let text_x = left + POPUP_PADDING + POPUP_ACCENT_W;

        // Heading: draw each base segment, with its furigana centred above.
        let word_y = top + POPUP_PADDING + if has_furi { furi_h + RUBY_GAP } else { 0 };
        let furi_y = top + POPUP_PADDING;
        let mut x = text_x;
        for seg in &segs {
            let old_font = SelectObject(dc, HGDIOBJ(fonts.word.0));
            SetTextColor(dc, COLOR_WORD_TEXT.to_colorref());
            if !seg.text.is_empty() {
                let _ = TextOutW(dc, x, word_y, &seg.text);
            }
            SelectObject(dc, old_font);

            if let Some(furi) = &seg.furi {
                let fx = x + (seg.width - seg.furi_w) / 2;
                let old_font = SelectObject(dc, HGDIOBJ(fonts.furi.0));
                SetTextColor(dc, COLOR_FURIGANA.to_colorref());
                let _ = TextOutW(dc, fx, furi_y, furi);
                SelectObject(dc, old_font);
            }
            x += seg.width;
        }

        // Body lines below the heading.
        let mut y = top + POPUP_PADDING + heading_h;
        for line in &body {
            y += LINE_GAP;
            let old_font = SelectObject(dc, HGDIOBJ(line.font.0));
            SetTextColor(dc, line.color);
            if !line.text.is_empty() {
                let _ = TextOutW(dc, text_x, y, &line.text);
            }
            SelectObject(dc, old_font);
            y += line.height;
        }

        draw_button(dc, &button);
    }

    // GDI leaves the alpha channel at 0; force the whole panel opaque (alpha
    // 255, where premultiplied == straight, so the GDI RGB is already correct).
    force_opaque(canvas.bits, frame_w, frame_h, &panel);

    (
        Rect::new(left as f32, top as f32, panel_w as f32, panel_h as f32),
        Rect::new(
            button.left as f32,
            button.top as f32,
            BUTTON_W as f32,
            BUTTON_H as f32,
        ),
    )
}

/// Draw the problem-report button: a small panel with a line-art camera icon.
unsafe fn draw_button(dc: HDC, button: &RECT) {
    unsafe {
        let bg = CreateSolidBrush(COLOR_BUTTON_BG.to_colorref());
        FillRect(dc, button, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let hollow = GetStockObject(HOLLOW_BRUSH);
        let old_brush = SelectObject(dc, hollow);

        let border = CreatePen(
            PS_SOLID,
            HAIRLINE_PEN_WIDTH,
            COLOR_BUTTON_BORDER.to_colorref(),
        );
        let old_pen = SelectObject(dc, HGDIOBJ(border.0));
        let _ = Rectangle(dc, button.left, button.top, button.right, button.bottom);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(border.0));

        // Camera icon in a light pen.
        let icon = CreatePen(
            PS_SOLID,
            HAIRLINE_PEN_WIDTH,
            COLOR_BUTTON_ICON.to_colorref(),
        );
        let old_pen = SelectObject(dc, HGDIOBJ(icon.0));
        let cx = (button.left + button.right) / 2;
        let body_top = button.top + 7;
        let _ = Rectangle(
            dc,
            button.left + 5,
            body_top,
            button.right - 5,
            button.bottom - 3,
        );
        let _ = Rectangle(dc, cx - 3, button.top + 4, cx + 3, body_top);
        let _ = Ellipse(dc, cx - 3, body_top + 2, cx + 3, button.bottom - 5);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(icon.0));

        SelectObject(dc, old_brush);
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

fn make_line(dc: HDC, text: &str, font: HFONT, color: COLORREF) -> PopupLine {
    let utf16 = utf16(text);
    let (width, height) = measure(dc, font, &utf16);
    PopupLine {
        text: utf16,
        font,
        color,
        width,
        height,
    }
}

/// Force every pixel in `rect` to alpha 255 (premultiplied with a==255 leaves
/// the GDI-written RGB unchanged), making the popup panel opaque.
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
