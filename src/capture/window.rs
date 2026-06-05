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

pub fn list_capturable_windows() -> Result<Vec<WindowInfo>> {
    let mut windows = Vec::new();
    for window in xcap::Window::all()? {
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        windows.push(WindowInfo {
            id: window.id().unwrap_or_default(),
            pid: window.pid().unwrap_or_default(),
            app_name: window.app_name().unwrap_or_default(),
            title: window.title().unwrap_or_default(),
            x: window.x().unwrap_or_default(),
            y: window.y().unwrap_or_default(),
            width: window.width().unwrap_or_default(),
            height: window.height().unwrap_or_default(),
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
