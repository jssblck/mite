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

#[tauri::command]
pub async fn download_gpu_pack(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let rel = release::fetch_latest()?;
        let gpu = rel
            .manifest
            .gpu_runtime
            .clone()
            .ok_or_else(|| anyhow::anyhow!("this release has no GPU runtime pack"))?;
        let url = rel
            .asset_url(&gpu.asset)
            .ok_or_else(|| anyhow::anyhow!("release is missing {}", gpu.asset))?;
        let home_dir = home::ensure_home()?;
        let tmp = home_dir.join(".gpu-runtime").join("pack.zip.tmp");
        download::download_to_file(&url, &tmp, |received, total| {
            emit_progress(&app, "gpu", &gpu.asset, received, total, false)
        })?;
        download::verify_sha256(&tmp, &gpu.sha256)?;
        download::extract_zip_all(&tmp, &home::gpu_runtime_dir()?)?;
        let _ = std::fs::remove_file(&tmp);
        emit_progress(&app, "gpu", &gpu.asset, 1, 1, true);
        Ok(())
    })
    .await
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

/// Remove the downloaded data (models, GPU pack, cache, config, logs) while
/// leaving the installed CLI binary in place. The frontend confirms first.
#[tauri::command]
pub async fn uninstall_data() -> Result<(), String> {
    blocking(|| {
        let home_dir = home::mite_home()?;
        for entry in ["models", "cache", ".gpu-runtime", "logs"] {
            let _ = std::fs::remove_dir_all(home_dir.join(entry));
        }
        let _ = std::fs::remove_file(home_dir.join("mite.toml"));
        Ok(())
    })
    .await
}
