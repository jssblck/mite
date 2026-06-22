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

/// Smallest edge, in logical pixels, of a window a reader would point Mite at.
/// Below this a "window" is a tray helper, a zero-size shell surface, or a
/// taskbar-thin sliver, never reading material.
const MIN_CAPTURE_EDGE: u32 = 200;
/// Widest long-edge / short-edge ratio we treat as a real window. The taskbar
/// and similar strips blow well past this; games and apps stay under it.
const MAX_ASPECT_RATIO: f64 = 6.0;

/// Window titles (case-insensitive, exact) that are always OS shell surfaces,
/// never a target a reader would choose.
const SHELL_TITLES: &[&str] = &[
    "windows explorer",
    "program manager",
    "windows input experience",
    "windows shell experience host",
    "nvidia geforce overlay",
    "search",
    "start",
];

/// Whether a window is an OS shell surface (the desktop, taskbar, a file
/// browser, a system overlay) rather than something worth reading.
fn is_shell_surface(title: &str, app_name: &str) -> bool {
    let title = title.trim().to_lowercase();
    let app = app_name.trim().to_lowercase();
    if SHELL_TITLES.contains(&title.as_str()) {
        return true;
    }
    // explorer.exe hosts both the desktop/taskbar and file-browser windows
    // ("<folder> - File Explorer"); a reader never points Mite at either.
    if title.ends_with("file explorer") {
        return true;
    }
    matches!(app.as_str(), "file explorer" | "windows explorer")
}

/// Whether a window is plausibly a capture target: large enough, a sane aspect
/// ratio, named, and not an OS shell surface. Pure so the policy is unit-tested
/// without enumerating real windows. Note this deliberately does not inspect
/// pixels: xcap's picker capture returns black for many GPU/DWM-composited apps
/// (Discord, Signal, Electron, games) that the WGC watch path captures fine, so
/// a "blank frame" filter would hide legitimate targets.
fn is_capture_target(title: &str, app_name: &str, width: u32, height: u32) -> bool {
    if title.trim().is_empty() && app_name.trim().is_empty() {
        return false;
    }
    if width < MIN_CAPTURE_EDGE || height < MIN_CAPTURE_EDGE {
        return false;
    }
    let (long, short) = if width >= height {
        (width, height)
    } else {
        (height, width)
    };
    if short == 0 || (long as f64 / short as f64) > MAX_ASPECT_RATIO {
        return false;
    }
    !is_shell_surface(title, app_name)
}

/// Non-minimized windows that are plausible capture targets, sorted by title.
/// Shell surfaces, slivers, tray helpers, and tiny windows are filtered out (see
/// [`is_capture_target`]).
pub fn list_windows() -> Result<Vec<WindowSummary>> {
    let mut out = Vec::new();
    for window in xcap::Window::all().context("enumerating windows")? {
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        let width = window.width().unwrap_or(0);
        let height = window.height().unwrap_or(0);
        let title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        if !is_capture_target(&title, &app_name, width, height) {
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
    out.sort_by_key(|w| w.title.to_lowercase());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_normal_application_window() {
        assert!(is_capture_target(
            "Some Visual Novel",
            "game.exe",
            1920,
            1080
        ));
        assert!(is_capture_target(
            "just-us - Discord",
            "Discord",
            3153,
            1812
        ));
        // Named only by app, no title, but a sane size is still a real window.
        assert!(is_capture_target("", "Signal", 2879, 1672));
    }

    #[test]
    fn rejects_unnamed_windows() {
        assert!(!is_capture_target("", "", 1920, 1080));
        assert!(!is_capture_target("   ", "  ", 1920, 1080));
    }

    #[test]
    fn rejects_tiny_windows() {
        // A 2x2 tray helper is never a reading target.
        assert!(!is_capture_target("Switch USB", "helper.exe", 2, 2));
        assert!(!is_capture_target("Sliver", "app.exe", 1920, 40));
    }

    #[test]
    fn rejects_insane_aspect_ratios() {
        // The taskbar reports as a full-width, very short strip.
        assert!(!is_capture_target("Taskbar", "explorer.exe", 3840, 72));
        // Tall-and-narrow is just as implausible as wide-and-short.
        assert!(!is_capture_target("Rail", "app.exe", 300, 2400));
        // A 6:1 panel is still allowed; just past it is not.
        assert!(is_capture_target("Wide editor", "app.exe", 2400, 400));
        assert!(!is_capture_target("Wider", "app.exe", 2401, 400));
    }

    #[test]
    fn rejects_shell_surfaces() {
        assert!(!is_capture_target(
            "Windows Explorer",
            "explorer.exe",
            3840,
            2160
        ));
        assert!(!is_capture_target(
            "Program Manager",
            "explorer.exe",
            3840,
            2160
        ));
        assert!(!is_capture_target(
            "NVIDIA GeForce Overlay",
            "nvidia.exe",
            3840,
            2160
        ));
        // File-browser windows carry a "<folder> - File Explorer" title.
        assert!(!is_capture_target(
            "projects - File Explorer",
            "",
            2254,
            1277
        ));
        // Matching is case-insensitive and ignores surrounding whitespace.
        assert!(!is_capture_target("  windows explorer  ", "", 1920, 1080));
        // Identified by app name when the title is something else.
        assert!(!is_capture_target("Downloads", "File Explorer", 1920, 1080));
    }
}
