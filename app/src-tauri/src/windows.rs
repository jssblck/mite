//! Window enumeration and live picker thumbnails via `xcap`.
//!
//! `xcap` is pinned to the same version the mite CLI uses, so the window `id`
//! here is the same HWND-derived id that `watch --window-id` consumes: the id
//! that drives a card's thumbnail also launches capture on that exact window.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSummary {
    pub id: u32,
    pub pid: u32,
    pub title: String,
    pub app_name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

/// Non-minimized, non-empty capturable windows, sorted by title.
pub fn list_windows() -> Result<Vec<WindowSummary>> {
    let mut out = Vec::new();
    for window in xcap::Window::all().context("enumerating windows")? {
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        let width = window.width().unwrap_or(0);
        let height = window.height().unwrap_or(0);
        if width == 0 || height == 0 {
            continue;
        }
        let title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        if title.is_empty() && app_name.is_empty() {
            continue;
        }
        out.push(WindowSummary {
            id: window.id().unwrap_or_default(),
            pid: window.pid().unwrap_or_default(),
            title,
            app_name,
            width,
            height,
            x: window.x().unwrap_or_default(),
            y: window.y().unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(out)
}

/// Capture window `window_id` and return a downscaled PNG as a `data:` URL,
/// suitable for an `<img src>` in the picker grid. `max_width` caps the long
/// edge so thumbnails stay cheap to encode and ship.
pub fn capture_thumbnail(window_id: u32, max_width: u32) -> Result<String> {
    let target = xcap::Window::all()
        .context("enumerating windows")?
        .into_iter()
        .find(|window| window.id().map(|id| id == window_id).unwrap_or(false))
        .context("window not found")?;

    if target.is_minimized().unwrap_or(false) {
        anyhow::bail!("window is minimized");
    }

    let image = target.capture_image().context("capturing window")?;
    let (width, height) = (image.width(), image.height());
    let max_width = max_width.clamp(64, 1024);
    let scaled = if width > max_width {
        let new_height = ((height as f64) * (max_width as f64 / width as f64))
            .round()
            .max(1.0) as u32;
        image::imageops::thumbnail(&image, max_width, new_height)
    } else {
        image
    };

    let mut png: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder;
        let encoder = image::codecs::png::PngEncoder::new(std::io::Cursor::new(&mut png));
        encoder
            .write_image(
                scaled.as_raw(),
                scaled.width(),
                scaled.height(),
                image::ExtendedColorType::Rgba8,
            )
            .context("encoding thumbnail PNG")?;
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(format!("data:image/png;base64,{encoded}"))
}
