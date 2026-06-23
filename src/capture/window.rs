use anyhow::{Context, Result, bail};

/// One or more criteria identifying a target window. The criteria combine as a
/// conjunction (a window must satisfy all that are set), and the fields are
/// private so the all-empty state, which matches nothing, is unrepresentable:
/// every `WindowSelector` carries at least one criterion by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSelector {
    title_substring: Option<String>,
    window_id: Option<u32>,
    pid: Option<u32>,
}

impl WindowSelector {
    pub fn title(title_substring: impl Into<String>) -> Self {
        Self {
            title_substring: Some(title_substring.into()),
            window_id: None,
            pid: None,
        }
    }

    /// Build a selector from optional CLI criteria, rejecting the empty case.
    /// This is the parse boundary: once constructed, the ">=1 criterion"
    /// invariant holds for the rest of the program.
    pub fn new(
        title_substring: Option<String>,
        window_id: Option<u32>,
        pid: Option<u32>,
    ) -> Result<Self> {
        if title_substring.is_none() && window_id.is_none() && pid.is_none() {
            bail!("at least one window selector is required: --title, --window-id, or --pid");
        }
        Ok(Self {
            title_substring,
            window_id,
            pid,
        })
    }

    /// The window-id criterion, if one was set. Used to decide whether an
    /// ambiguous (multi-window) match is worth warning about.
    pub fn window_id(&self) -> Option<u32> {
        self.window_id
    }

    /// Whether a window with the given (possibly unreadable) attributes
    /// satisfies every criterion that is set. An attribute that a criterion
    /// needs but that could not be read (`None`) fails that criterion.
    pub fn matches(&self, id: Option<u32>, pid: Option<u32>, title: Option<&str>) -> bool {
        self.window_id.is_none_or(|want| id == Some(want))
            && self.pid.is_none_or(|want| pid == Some(want))
            && self.title_substring.as_ref().is_none_or(|needle| {
                title.is_some_and(|title| title.to_lowercase().contains(&needle.to_lowercase()))
            })
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(title) = &self.title_substring {
            parts.push(format!("title contains {title:?}"));
        }
        if let Some(window_id) = self.window_id {
            parts.push(format!("window-id={window_id}"));
        }
        if let Some(pid) = self.pid {
            parts.push(format!("pid={pid}"));
        }
        parts.join(", ")
    }
}

/// Smallest edge, in pixels, of a window a reader would point Mite at. Below
/// this a "window" is a tray helper, a zero-size shell surface, or a taskbar-thin
/// sliver, never reading material.
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
/// without enumerating real windows.
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
/// [`is_capture_target`]) so both the `list-windows` diagnostic and the desktop
/// picker that consumes it see only windows a reader would actually point Mite
/// at.
pub fn list_capturable_windows() -> Result<Vec<WindowInfo>> {
    let mut windows = Vec::new();
    for window in xcap::Window::all()? {
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        let title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        let width = window.width().unwrap_or_default();
        let height = window.height().unwrap_or_default();
        if !is_capture_target(&title, &app_name, width, height) {
            continue;
        }
        windows.push(WindowInfo {
            id: window.id().unwrap_or_default(),
            pid: window.pid().unwrap_or_default(),
            app_name,
            title,
            x: window.x().unwrap_or_default(),
            y: window.y().unwrap_or_default(),
            width,
            height,
        });
    }
    windows.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(windows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u32,
    pub pid: u32,
    pub app_name: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowInfo {
    pub(super) fn from_window(window: &xcap::Window) -> Result<Self> {
        Ok(Self {
            id: window.id().unwrap_or_default(),
            pid: window.pid().unwrap_or_default(),
            app_name: window.app_name().unwrap_or_default(),
            title: window.title().unwrap_or_default(),
            x: window.x().unwrap_or_default(),
            y: window.y().unwrap_or_default(),
            width: window.width()?,
            height: window.height()?,
        })
    }
}

pub(super) fn find_window(selector: &WindowSelector) -> Result<xcap::Window> {
    let mut matches = xcap::Window::all()?
        .into_iter()
        .filter(|window| !window.is_minimized().unwrap_or(true))
        .filter(|window| {
            selector.matches(
                window.id().ok(),
                window.pid().ok(),
                window.title().ok().as_deref(),
            )
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|window| window.id().unwrap_or_default());
    if matches.len() > 1 && selector.window_id().is_none() {
        tracing::warn!(
            "window selector matched {} windows; using the first by window id. Selector: {}",
            matches.len(),
            selector.describe()
        );
    }
    matches
        .into_iter()
        .next()
        .with_context(|| format!("no non-minimized window matched {}", selector.describe()))
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
