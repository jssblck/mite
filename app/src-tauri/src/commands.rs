//! The Tauri command surface the frontend invokes.
//!
//! Long-running, blocking work (HTTP downloads, archive extraction, spawning
//! the CLI for doctor) runs on a blocking thread via `spawn_blocking` so the
//! async IPC stays responsive, and streams progress as `download-progress`
//! events. Errors are flattened to strings for the frontend.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

use crate::release::{self, normalize_version};
use crate::settings::{self, AppSettings};
use crate::status::{self, AppStatus};
use crate::watch::{self, WatchOptions, WatchState};
use crate::{cli, download, home, models, windows};

/// Run blocking work off the async executor and flatten errors to strings.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|join| join.to_string())?
        .map_err(|err| format!("{err:#}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    task: &'a str,
    file: &'a str,
    received: u64,
    total: u64,
    done: bool,
}

fn emit_progress(app: &AppHandle, task: &str, file: &str, received: u64, total: u64, done: bool) {
    let _ = app.emit(
        "download-progress",
        Progress {
            task,
            file,
            received,
            total,
            done,
        },
    );
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_cli: Option<String>,
    pub latest_tag: Option<String>,
    pub latest_cli: Option<String>,
    pub cli_update_available: bool,
    pub app_version: String,
}

#[tauri::command]
pub fn app_version() -> String {
    env!("APP_VERSION").to_string()
}

#[tauri::command]
pub fn mite_home_path() -> Result<String, String> {
    home::mite_home()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub async fn get_status() -> Result<AppStatus, String> {
    blocking(status::collect).await
}

#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateInfo, String> {
    blocking(|| {
        let current_cli = cli::installed_version();
        let latest = release::fetch_latest().ok();
        let latest_tag = latest.as_ref().map(|rel| rel.tag.clone());
        let latest_cli = latest.as_ref().map(|rel| rel.manifest.version.clone());
        let cli_update_available = match (&current_cli, &latest_cli) {
            (Some(current), Some(latest)) => {
                normalize_version(current) != normalize_version(latest)
            }
            (None, Some(_)) => true,
            _ => false,
        };
        Ok(UpdateInfo {
            current_cli,
            latest_tag,
            latest_cli,
            cli_update_available,
            app_version: env!("APP_VERSION").to_string(),
        })
    })
    .await
}

#[tauri::command]
pub async fn install_or_update_cli(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let rel = release::fetch_latest()?;
        let url = rel
            .asset_url(&rel.manifest.cli.asset)
            .ok_or_else(|| anyhow::anyhow!("release is missing {}", rel.manifest.cli.asset))?;
        let bin_dir = home::ensure_home()?.join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let dest = bin_dir.join("mite.exe");
        download::download_to_file(&url, &dest, |received, total| {
            emit_progress(&app, "cli", &rel.manifest.cli.asset, received, total, false)
        })?;
        download::verify_sha256(&dest, &rel.manifest.cli.sha256)?;
        emit_progress(&app, "cli", &rel.manifest.cli.asset, 1, 1, true);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn download_models(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let rel = release::fetch_latest()?;
        let manifest_url = rel
            .asset_url(&rel.manifest.model_manifest.asset)
            .ok_or_else(|| {
                anyhow::anyhow!("release is missing {}", rel.manifest.model_manifest.asset)
            })?;
        let manifest: models::ModelManifest = download::get_json(&manifest_url)?;
        let home_dir = home::ensure_home()?;
        models::download_all(&home_dir, &manifest, |id, received, total, done| {
            emit_progress(&app, "models", id, received, total, done)
        })?;
        Ok(())
    })
    .await
}

/// Run the NVIDIA runtime detection and return the full `mite doctor --json`
/// report (its `nvidia` and `gpu_runtime` fields drive the guided setup). The
/// guided setup screen polls this on a short interval so each component checks
/// off live as the user installs it.
#[tauri::command]
pub async fn detect_runtime() -> Result<serde_json::Value, String> {
    blocking(cli::doctor_json).await
}

/// Re-run detection and persist the detected tier and DLL directories, marking
/// the guided setup as seen. Called when the user finishes or skips the guided
/// flow, and silently on first launch when there is no NVIDIA GPU. The launcher
/// reads this to choose the backend and DLL search path.
#[tauri::command]
pub async fn record_runtime() -> Result<AppSettings, String> {
    blocking(|| {
        let report = cli::doctor_json()?;
        let gpu = report.get("gpu_runtime");
        let runtime_tier = gpu
            .and_then(|value| value.get("tier"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let dll_dirs = gpu
            .and_then(|value| value.get("dll_dirs"))
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let saved = AppSettings {
            runtime_tier,
            dll_dirs,
            runtime_setup_seen: true,
        };
        settings::save(&saved)?;
        Ok(saved)
    })
    .await
}

/// The persisted app settings (recorded runtime tier, DLL dirs, seen flag).
#[tauri::command]
pub async fn get_settings() -> Result<AppSettings, String> {
    blocking(|| Ok(settings::load())).await
}

#[tauri::command]
pub async fn write_default_config() -> Result<(), String> {
    blocking(|| {
        home::ensure_home()?;
        cli::write_default_config()
    })
    .await
}

#[tauri::command]
pub async fn list_windows() -> Result<Vec<windows::WindowSummary>, String> {
    blocking(windows::list_windows).await
}

#[tauri::command]
pub async fn capture_thumbnail(window_id: u32, max_width: u32) -> Result<String, String> {
    blocking(move || windows::capture_thumbnail(window_id, max_width)).await
}

#[tauri::command]
pub fn start_watch(
    app: AppHandle,
    state: State<WatchState>,
    options: WatchOptions,
) -> Result<(), String> {
    watch::start(&app, &state, options).map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub fn stop_watch(state: State<WatchState>) -> Result<(), String> {
    watch::stop(&state).map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub fn is_watching(state: State<WatchState>) -> bool {
    watch::is_watching(&state)
}

#[tauri::command]
pub fn open_mite_home(app: AppHandle) -> Result<(), String> {
    let home_dir = home::ensure_home().map_err(|err| format!("{err:#}"))?;
    app.opener()
        .open_path(home_dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| err.to_string())
}

/// Open a URL in the user's default browser (the NVIDIA download pages the
/// guided runtime setup links to).
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| err.to_string())
}

/// Remove the downloaded data (models, engine cache, config, logs) while leaving
/// the installed CLI binary in place. This never touches NVIDIA's runtime: Mite
/// does not install those bytes, so it does not delete them either. The frontend
/// confirms first.
#[tauri::command]
pub async fn uninstall_data() -> Result<(), String> {
    blocking(|| {
        let home_dir = home::mite_home()?;
        for entry in ["models", "cache", "logs"] {
            let _ = std::fs::remove_dir_all(home_dir.join(entry));
        }
        let _ = std::fs::remove_file(home_dir.join("mite.toml"));
        Ok(())
    })
    .await
}
