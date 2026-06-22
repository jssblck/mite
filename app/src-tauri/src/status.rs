//! The aggregate readiness snapshot the UI renders on every screen.

use anyhow::Result;
use serde::Serialize;

use crate::{cli, home, settings};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// Absolute path to the mite home directory.
    pub mite_home: String,
    /// The app's own version (git-tag derived at build time).
    pub app_version: String,
    /// Whether `bin\mite.exe` is installed.
    pub cli_installed: bool,
    /// The installed CLI version string, if any.
    pub cli_version: Option<String>,
    /// Whether the core model/dictionary files are present.
    pub models_ready: bool,
    /// Whether the guided NVIDIA runtime setup has been completed or skipped.
    /// Drives whether the app auto-opens the guided flow on launch.
    pub runtime_setup_seen: bool,
    /// The parsed `mite doctor --json` report, when the CLI and models are ready.
    /// Its `gpu_runtime` field carries the detected tier and per-DLL presence.
    pub doctor: Option<serde_json::Value>,
}

/// Collect the current install state. Cheap enough to call on focus/refresh;
/// only runs `doctor` when the CLI and models are present.
pub fn collect() -> Result<AppStatus> {
    let mite_home = home::mite_home()?.to_string_lossy().to_string();
    let cli_version = cli::installed_version();
    let cli_installed = cli_version.is_some();
    let models_ready = home::models_ready();
    let runtime_setup_seen = settings::load().runtime_setup_seen;
    let doctor = if cli_installed && models_ready {
        cli::doctor_json().ok()
    } else {
        None
    };

    Ok(AppStatus {
        mite_home,
        app_version: env!("APP_VERSION").to_string(),
        cli_installed,
        cli_version,
        models_ready,
        runtime_setup_seen,
        doctor,
    })
}
