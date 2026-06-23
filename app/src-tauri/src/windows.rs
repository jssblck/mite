//! Picker window list, sourced from the mite CLI's `list-windows`.
//!
//! The app does not enumerate or capture windows itself: it shells out to the
//! installed CLI (`mite list-windows --json --thumbnails`), which captures each
//! window with the same WGC engine the watch path uses. That is what makes the
//! preview honest: GPU/DWM-composited apps (Signal, Discord, Chromium, games)
//! that a GDI readback returns black get a real thumbnail, and the window `id`
//! is the same HWND `watch --window-id` consumes, because both come from the one
//! CLI. The picker is only reachable once the CLI is installed (the app shell
//! gates on it), so there is no pre-install path through here.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli;

/// Long-edge cap, in pixels, for picker thumbnails. Small enough to stay cheap
/// to capture, encode, and ship to the webview; large enough to recognize the
/// window at the card's render size.
const THUMBNAIL_MAX_WIDTH: u32 = 360;

/// One window in the picker. Deserialized from the CLI's `list-windows --json`
/// output and re-serialized to the frontend unchanged, so the field names are
/// camelCase on both sides.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// WGC thumbnail as a PNG data URL. With `--thumbnails` the CLI only returns
    /// windows it could capture a usable frame from, so this is populated in
    /// practice; it stays optional for the plain `--json` listing, which carries
    /// no thumbnails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// Run `mite list-windows --json --thumbnails` and parse the result. The CLI
/// captures every window's thumbnail in parallel, so this is a single round trip
/// for the whole grid rather than one capture call per card.
pub fn list_windows() -> Result<Vec<WindowSummary>> {
    let output = cli::command()?
        .arg("list-windows")
        .arg("--json")
        .arg("--thumbnails")
        .arg("--thumbnail-max-width")
        .arg(THUMBNAIL_MAX_WIDTH.to_string())
        .output()
        .context("running mite list-windows")?;
    if !output.status.success() {
        bail!(
            "mite list-windows failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing list-windows JSON")
}
