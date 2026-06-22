//! Headless preview of the overlay + popup rendering.
//!
//! Draws a synthetic "game" scene (a dark VN-style text box with a Japanese
//! sentence), feeds the same glyph rectangles to the real [`Win32Overlay`] so it
//! paints its furigana, category underlines, and definition popups exactly as it
//! would live, then composites the overlay over the scene and writes PNGs under
//! `target/overlay-preview/`. This is a developer tool for eyeballing overlay
//! presentation without a running game; it never ships in the product path.
//!
//!   cargo run --example overlay_preview

use std::ffi::c_void;
use std::path::PathBuf;

use anyhow::{Context, Result};
use image::RgbImage;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FillRect,
    GetTextExtentPoint32W, HGDIOBJ, OUT_TT_PRECIS, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, TextOutW,
};
use windows::core::PCWSTR;

use mite::geometry::{Rect, ScreenRect, Size};
use mite::hover::{FuriSegment, Highlight, PopupContent, WordCategory};
use mite::win32_overlay::{Popup, Win32Overlay};

const SCENE_W: i32 = 1040;
const SCENE_H: i32 = 620;
const BASE_FONT_H: i32 = -52;
const WORD_GAP: i32 = 16;
const LINE_TOP: [i32; 3] = [96, 268, 440];
const LEFT_MARGIN: i32 = 96;

/// A scripted word before layout: surface, category, known-ness, surface ruby.
type WordSpec = (&'static str, WordCategory, bool, Vec<FuriSegment>);

/// One laid-out word: where its glyphs were drawn, plus the data the overlay
/// needs to decorate it.
struct Word {
    rect: Rect,
    category: WordCategory,
    known: bool,
    ruby: Vec<FuriSegment>,
}

fn seg(text: &str, furi: Option<&str>) -> FuriSegment {
    FuriSegment {
        text: text.to_string(),
        furigana: furi.map(str::to_string),
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() -> Result<()> {
    // (surface, category, known, ruby) per word, grouped into three lines.
    let lines: [Vec<WordSpec>; 3] = [
        vec![
            (
                "母",
                WordCategory::Noun,
                true,
                vec![seg("母", Some("はは"))],
            ),
            ("は", WordCategory::Particle, true, vec![seg("は", None)]),
            (
                "寒い",
                WordCategory::Adjective,
                true,
                vec![seg("寒", Some("さむ")), seg("い", None)],
            ),
            (
                "朝",
                WordCategory::Noun,
                true,
                vec![seg("朝", Some("あさ"))],
            ),
            ("に", WordCategory::Particle, true, vec![seg("に", None)]),
            (
                "とても",
                WordCategory::Adverb,
                true,
                vec![seg("とても", None)],
            ),
            (
                "熱い",
                WordCategory::Adjective,
                true,
                vec![seg("熱", Some("あつ")), seg("い", None)],
            ),
        ],
        vec![
            (
                "コーヒー",
                WordCategory::Noun,
                true,
                vec![seg("コーヒー", None)],
            ),
            ("を", WordCategory::Particle, true, vec![seg("を", None)]),
            (
                "ゆっくり",
                WordCategory::Adverb,
                true,
                vec![seg("ゆっくり", None)],
            ),
        ],
        vec![
            (
                "飲みます",
                WordCategory::Verb,
                true,
                vec![seg("飲", Some("の")), seg("みます", None)],
            ),
            ("。", WordCategory::Other, false, vec![seg("。", None)]),
        ],
    ];

    let (scene, words) = draw_scene(&lines)?;
    let names: Vec<&str> = words_surfaces(&lines);

    let out_dir = PathBuf::from("target/overlay-preview");
    std::fs::create_dir_all(&out_dir)?;

    let highlights: Vec<Highlight> = words
        .iter()
        .map(|w| Highlight {
            rect: w.rect,
            category: w.category,
            known: w.known,
            ruby: w.ruby.clone(),
        })
        .collect();

    let mut overlay = Win32Overlay::new()?;
    overlay.set_furigana_visible(true); // preview the opt-in reading aid
    let screen = ScreenRect::new(0, 0, Size::new(SCENE_W as u32, SCENE_H as u32));
    overlay.present_snapshot(screen, &highlights);
    save_composite(&scene, &overlay, out_dir.join("scene.png"))?;

    // Hover each of three representative words and capture its popup.
    for (surface, content) in [
        ("飲みます", verb_popup()),
        ("寒い", adjective_popup()),
        ("朝", noun_popup()),
    ] {
        let index = names.iter().position(|&s| s == surface).context("word")?;
        overlay.set_interaction(
            Some(index),
            Some(Popup {
                word_rect: words[index].rect,
                content,
            }),
        );
        let file = out_dir.join(format!("popup-{surface}.png"));
        save_composite(&scene, &overlay, file)?;
        // Reset so the next snapshot starts clean.
        overlay.present_snapshot(screen, &highlights);
    }

    // "Invisible" mode: word underlines and furigana both off. The word layer is
    // transparent, but hovering still raises the popup.
    overlay.set_underlines_visible(false);
    overlay.set_furigana_visible(false);
    let index = names.iter().position(|&s| s == "朝").context("word")?;
    overlay.set_interaction(
        Some(index),
        Some(Popup {
            word_rect: words[index].rect,
            content: noun_popup(),
        }),
    );
    save_composite(&scene, &overlay, out_dir.join("invisible-mode.png"))?;

    println!("wrote previews to {}", out_dir.display());
    Ok(())
}

fn words_surfaces(lines: &[Vec<WordSpec>; 3]) -> Vec<&'static str> {
    lines.iter().flatten().map(|w| w.0).collect()
}

/// Draw the synthetic scene and return it as an image plus the per-word glyph
/// rectangles the overlay should decorate.
fn draw_scene(lines: &[Vec<WordSpec>; 3]) -> Result<(RgbImage, Vec<Word>)> {
    let mut bits: *mut c_void = std::ptr::null_mut();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SCENE_W,
            biHeight: -SCENE_H, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut words = Vec::new();
    let scene = unsafe {
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .context("CreateDIBSection failed")?;
        let dc = CreateCompatibleDC(None);
        SelectObject(dc, HGDIOBJ(bitmap.0));

        // Background, then a rounded VN-style text box.
        fill(dc, 0, 0, SCENE_W, SCENE_H, COLORREF(0x0014_1414));
        let panel = CreateSolidBrush(COLORREF(0x0020_1d1d));
        let _ = RoundRect(dc, 24, 24, SCENE_W - 24, SCENE_H - 24, 40, 40);
        let _ = SelectObject(dc, HGDIOBJ(panel.0));
        let _ = RoundRect(dc, 24, 24, SCENE_W - 24, SCENE_H - 24, 40, 40);
        let _ = DeleteObject(HGDIOBJ(panel.0));

        let font = CreateFontW(
            BASE_FONT_H,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(utf16("Yu Gothic UI").as_ptr()),
        );
        let old_font = SelectObject(dc, HGDIOBJ(font.0));
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00f5_f3f0));

        for (line, top) in lines.iter().zip(LINE_TOP) {
            let mut x = LEFT_MARGIN;
            for (surface, category, known, ruby) in line {
                let text: Vec<u16> = surface.encode_utf16().collect();
                let mut size = Default::default();
                let _ = GetTextExtentPoint32W(dc, &text, &mut size);
                let _ = TextOutW(dc, x, top, &text);
                words.push(Word {
                    rect: Rect::new(x as f32, top as f32, size.cx as f32, size.cy as f32),
                    category: *category,
                    known: *known,
                    ruby: ruby.clone(),
                });
                x += size.cx + WORD_GAP;
            }
        }

        SelectObject(dc, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));

        let len = (SCENE_W * SCENE_H * 4) as usize;
        let raw = std::slice::from_raw_parts(bits as *const u8, len);
        let mut img = RgbImage::new(SCENE_W as u32, SCENE_H as u32);
        for (i, px) in img.pixels_mut().enumerate() {
            let o = i * 4;
            *px = image::Rgb([raw[o + 2], raw[o + 1], raw[o]]);
        }
        let _ = DeleteDC(dc);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        img
    };

    Ok((scene, words))
}

fn fill(dc: windows::Win32::Graphics::Gdi::HDC, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let rect = windows::Win32::Foundation::RECT {
            left: x,
            top: y,
            right: w,
            bottom: h,
        };
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// Composite the overlay's premultiplied BGRA surface over the scene and save.
fn save_composite(scene: &RgbImage, overlay: &Win32Overlay, path: PathBuf) -> Result<()> {
    let (ow, oh, bgra) = overlay
        .overlay_surface()
        .context("overlay has no surface")?;
    let mut out = scene.clone();
    let stride = ow.max(0) as usize * 4;
    let width = (scene.width() as i32).min(ow).max(0) as u32;
    let height = (scene.height() as i32).min(oh).max(0) as u32;
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let o = row + x as usize * 4;
            let (b, g, r, a) = (bgra[o], bgra[o + 1], bgra[o + 2], bgra[o + 3]);
            if a == 0 {
                continue;
            }
            let inv = (255 - a) as u32;
            let p = out.get_pixel_mut(x, y);
            p.0[0] = (r as u32 + p.0[0] as u32 * inv / 255).min(255) as u8;
            p.0[1] = (g as u32 + p.0[1] as u32 * inv / 255).min(255) as u8;
            p.0[2] = (b as u32 + p.0[2] as u32 * inv / 255).min(255) as u8;
        }
    }
    out.save(&path)
        .with_context(|| format!("save {}", path.display()))?;
    Ok(())
}

fn verb_popup() -> PopupContent {
    PopupContent {
        word: "飲む".to_string(),
        ruby: vec![seg("飲", Some("の")), seg("む", None)],
        note: Some("Polite masu-form: the polite non-past of 飲む.".to_string()),
        glosses: vec!["to drink  (v5m)".to_string()],
        category: WordCategory::Verb,
        pill: Some("VERB (GODAN)".to_string()),
    }
}

fn adjective_popup() -> PopupContent {
    PopupContent {
        word: "寒い".to_string(),
        ruby: vec![seg("寒", Some("さむ")), seg("い", None)],
        note: None,
        glosses: vec!["cold  (adj-i)".to_string()],
        category: WordCategory::Adjective,
        pill: Some("I-ADJECTIVE".to_string()),
    }
}

fn noun_popup() -> PopupContent {
    PopupContent {
        word: "朝".to_string(),
        ruby: vec![seg("朝", Some("あさ"))],
        note: None,
        glosses: vec!["morning  (n)".to_string()],
        category: WordCategory::Noun,
        pill: Some("NOUN".to_string()),
    }
}
