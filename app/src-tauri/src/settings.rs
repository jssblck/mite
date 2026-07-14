//! Persistent app settings stored alongside the mite home.
//!
//! The guided NVIDIA runtime setup records what it detected here so the app can
//! launch the CLI with the right backend and DLL search path, and so it does not
//! reopen the guided flow on every launch. Mite never installs the NVIDIA
//! binaries; this only remembers where the user installed them and which tier
//! that supports.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::home;

/// The persisted app settings file inside the mite home.
const SETTINGS_FILE: &str = "app-settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// The recorded runtime tier: `"cpu"`, `"cuda"`, or `"tensor_rt"`. `None`
    /// until the app records a detection.
    pub runtime_tier: Option<String>,
    /// Directories that held the detected NVIDIA DLLs. Prepended to the launched
    /// CLI's `PATH` so the OS loader resolves the runtime the user installed.
    pub dll_dirs: Vec<String>,
    /// True once the guided runtime setup has been completed or skipped, so the
    /// app does not reopen it automatically on later launches.
    pub runtime_setup_seen: bool,
    /// Run the overlay continuously (`watch --auto`) instead of holding Shift.
    /// On by default: some games swallow the Shift hotkey.
    pub watch_auto: bool,
    /// Draw the overlay only while the watched window is focused
    /// (`watch --focus-only`; the picker always pins a window id, which that
    /// flag requires). On by default: without it, an app-launched overlay in
    /// continuous mode keeps drawing over whatever the user alt-tabs to.
    pub watch_focus_only: bool,
    /// Show the per-stage latency HUD (`watch --hud`).
    pub watch_hud: bool,
    /// Log aggregate metrics every N seconds (`watch --metrics-interval-secs`);
    /// `0` disables it.
    pub watch_metrics_interval_secs: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            runtime_tier: None,
            dll_dirs: Vec::new(),
            runtime_setup_seen: false,
            // Matches the previous in-picker defaults: continuous on, the
            // diagnostic surfaces off. Focus gating is on so the continuous
            // overlay disappears while the watched window is unfocused.
            watch_auto: true,
            watch_focus_only: true,
            watch_hud: false,
            watch_metrics_interval_secs: 0,
        }
    }
}

impl AppSettings {
    /// The CLI `--backend` value for the recorded tier, or `None` to leave the
    /// config default (which auto-degrades to CPU when no runtime is present).
    pub fn backend_flag(&self) -> Option<&'static str> {
        match self.runtime_tier.as_deref() {
            Some("tensor_rt") => Some("nvidia_tensor_rt_then_cuda"),
            Some("cuda") => Some("cuda"),
            Some("cpu") => Some("cpu"),
            _ => None,
        }
    }
}

fn settings_path() -> Result<PathBuf> {
    Ok(home::mite_home()?.join(SETTINGS_FILE))
}

/// Load the saved settings, or defaults when the file is absent or unreadable.
pub fn load() -> AppSettings {
    let Ok(path) = settings_path() else {
        return AppSettings::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AppSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Persist the settings to the mite home.
pub fn save(settings: &AppSettings) -> Result<()> {
    home::ensure_home()?;
    let path = settings_path()?;
    let text = serde_json::to_string_pretty(settings).context("serializing app settings")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_flag_maps_recorded_tier() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.backend_flag(), None);

        settings.runtime_tier = Some("tensor_rt".to_string());
        assert_eq!(settings.backend_flag(), Some("nvidia_tensor_rt_then_cuda"));

        settings.runtime_tier = Some("cuda".to_string());
        assert_eq!(settings.backend_flag(), Some("cuda"));

        settings.runtime_tier = Some("cpu".to_string());
        assert_eq!(settings.backend_flag(), Some("cpu"));

        settings.runtime_tier = Some("nonsense".to_string());
        assert_eq!(settings.backend_flag(), None);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = AppSettings {
            runtime_tier: Some("cuda".to_string()),
            dll_dirs: vec!["C:\\nvidia\\bin".to_string()],
            runtime_setup_seen: true,
            watch_auto: false,
            watch_focus_only: false,
            watch_hud: true,
            watch_metrics_interval_secs: 5,
        };
        let text = serde_json::to_string(&settings).unwrap();
        let decoded: AppSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded.runtime_tier.as_deref(), Some("cuda"));
        assert_eq!(decoded.dll_dirs, vec!["C:\\nvidia\\bin".to_string()]);
        assert!(decoded.runtime_setup_seen);
        assert!(!decoded.watch_auto);
        assert!(!decoded.watch_focus_only);
        assert!(decoded.watch_hud);
        assert_eq!(decoded.watch_metrics_interval_secs, 5);
    }

    #[test]
    fn watch_defaults_fill_in_for_settings_files_predating_the_fields() {
        // A settings file written before watch options existed must deserialize
        // with continuous-watch on, matching the prior in-picker default, and
        // with focus gating on (the default for files predating that field).
        let legacy = r#"{"runtimeTier":"cpu","dllDirs":[],"runtimeSetupSeen":true}"#;
        let decoded: AppSettings = serde_json::from_str(legacy).unwrap();
        assert!(decoded.watch_auto);
        assert!(decoded.watch_focus_only);
        assert!(!decoded.watch_hud);
        assert_eq!(decoded.watch_metrics_interval_secs, 0);
    }
}
