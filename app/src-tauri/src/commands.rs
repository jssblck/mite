//! The Tauri command surface the frontend invokes.
//!
//! Long-running, blocking work (HTTP downloads, archive extraction, spawning
//! the CLI for doctor) runs on a blocking thread via `spawn_blocking` so the
//! async IPC stays responsive, and streams progress as `download-progress`
//! events. Errors are flattened to strings for the frontend.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

use crate::engine_use::EngineUse;
use crate::release::{self, normalize_version};
use crate::settings::{self, AppSettings};
use crate::status::{self, AppStatus};
use crate::watch::{self, WatchState};
use crate::{cli, download, home, models, warmup, windows};

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

/// How the installed engine relates to the engine this app build wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    /// The newest compatible engine is installed; nothing to do.
    Ok,
    /// A newer engine within the app's compatible range is available.
    Update,
    /// The engine is missing or outside the app's compatible range (for example
    /// the app self-updated past it); it must be reconciled before mite runs.
    Required,
    /// The target engine could not be resolved (offline, or no compatible
    /// release found); don't act on it.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub app_version: String,
    /// The installed engine version, if the CLI is present.
    pub current_cli: Option<String>,
    /// The engine version this app build should run (newest compatible).
    pub target_cli: Option<String>,
    /// The release tag the target engine comes from.
    pub target_tag: Option<String>,
    pub engine_state: EngineState,
}

/// Classify the installed engine against the target (newest compatible) engine.
///
/// Pure so it can be unit tested without a release feed. `app_version` is used to
/// decide whether the *installed* engine is still inside the app's caret range:
/// when it is not (e.g. the app self-updated to a new minor), reconciling is
/// required rather than a mere optional update.
fn engine_state(app_version: &str, current: Option<&str>, target: Option<&str>) -> EngineState {
    let Some(target) = target.and_then(release::parse_version) else {
        return EngineState::Unknown;
    };
    let Some(current) = current else {
        return EngineState::Required;
    };
    let Some(current) = release::parse_version(current) else {
        return EngineState::Required;
    };
    if let Some(req) = release::engine_requirement(app_version) {
        if !req.matches(&current) {
            return EngineState::Required;
        }
    }
    if current < target {
        EngineState::Update
    } else {
        EngineState::Ok
    }
}

/// Verify a freshly downloaded file's sha256, deleting it on mismatch.
///
/// `download::download_to_file` renames its `.part` into the final path before we
/// can check the hash, so a corrupt-but-complete download would otherwise linger
/// at the destination. Since the self-heal check (`sidecars_present`) only tests
/// for existence, that stale bad file would mask the problem. Removing it on
/// failure means the next reconcile re-fetches.
fn verify_or_discard(dest: &std::path::Path, sha256: &str) -> anyhow::Result<()> {
    download::verify_sha256(dest, sha256).inspect_err(|_| {
        let _ = std::fs::remove_file(dest);
    })
}

/// Whether every declared engine sidecar is present in `bin_dir`.
///
/// Presence-only by design: hashing the (large) provider DLLs on every status
/// poll would be wasteful, and the install path verifies each sidecar's sha256
/// when it writes it. This only needs to catch the missing-file case that the
/// version check cannot see.
fn sidecars_present(bin_dir: &std::path::Path, extra_files: &[release::AssetRef]) -> bool {
    extra_files.iter().all(|extra| {
        std::path::Path::new(&extra.asset)
            .file_name()
            .is_some_and(|name| bin_dir.join(name).is_file())
    })
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
        let app_version = env!("APP_VERSION").to_string();
        let current_cli = cli::installed_version();
        // Resolve the engine compatible with this app build, not always-latest.
        let resolved = release::resolve_engine_release(&app_version).ok().flatten();
        let target_tag = resolved.as_ref().map(|rel| rel.tag.clone());
        let target_cli = resolved
            .as_ref()
            .map(|rel| normalize_version(&rel.manifest.version).to_string());
        let mut engine_state =
            engine_state(&app_version, current_cli.as_deref(), target_cli.as_deref());
        // Self-heal an incomplete engine. The version check alone treats a
        // matching CLI as done, but the engine also needs its sidecar provider
        // DLLs next to mite.exe. They can be absent when an interrupted install
        // wrote mite.exe but not the DLLs, or when upgrading from a pre-sidecar
        // engine whose version already equals the target. In both cases force a
        // reconcile so the install path (re)fetches the sidecars.
        if engine_state == EngineState::Ok {
            if let (Some(rel), Ok(home_dir)) = (resolved.as_ref(), home::mite_home()) {
                if !sidecars_present(&home_dir.join("bin"), &rel.manifest.cli.extra_files) {
                    engine_state = EngineState::Required;
                }
            }
        }
        Ok(UpdateInfo {
            app_version,
            current_cli,
            target_cli,
            target_tag,
            engine_state,
        })
    })
    .await
}

#[tauri::command]
pub async fn install_or_update_cli(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let app_version = env!("APP_VERSION");
        let rel = release::resolve_engine_release(app_version)?.ok_or_else(|| {
            anyhow::anyhow!("no engine release compatible with app {app_version} was found")
        })?;
        let url = rel
            .asset_url(&rel.manifest.cli.asset)
            .ok_or_else(|| anyhow::anyhow!("release is missing {}", rel.manifest.cli.asset))?;
        let bin_dir = home::ensure_home()?.join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let dest = bin_dir.join("mite.exe");
        download::download_to_file(&url, &dest, |received, total| {
            emit_progress(&app, "cli", &rel.manifest.cli.asset, received, total, false)
        })?;
        verify_or_discard(&dest, &rel.manifest.cli.sha256)?;
        emit_progress(&app, "cli", &rel.manifest.cli.asset, 1, 1, true);

        // Install the engine sidecars next to mite.exe. These are ONNX Runtime's
        // provider bridge DLLs; without them the engine cannot register a GPU
        // execution provider and silently falls back to the CPU. Older releases
        // have no extra_files, so this loop is a no-op for them.
        for extra in &rel.manifest.cli.extra_files {
            let file_name = std::path::Path::new(&extra.asset)
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid engine asset name {}", extra.asset))?;
            let url = rel
                .asset_url(&extra.asset)
                .ok_or_else(|| anyhow::anyhow!("release is missing {}", extra.asset))?;
            let dest = bin_dir.join(file_name);
            download::download_to_file(&url, &dest, |received, total| {
                emit_progress(&app, "cli", &extra.asset, received, total, false)
            })?;
            verify_or_discard(&dest, &extra.sha256)?;
            emit_progress(&app, "cli", &extra.asset, 1, 1, true);
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn download_models(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let app_version = env!("APP_VERSION");
        // Models are versioned with the engine, so pin them to the same
        // compatible release the engine comes from.
        let rel = release::resolve_engine_release(app_version)?.ok_or_else(|| {
            anyhow::anyhow!("no engine release compatible with app {app_version} was found")
        })?;
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
        // Preserve any watch options the user has set; only the runtime fields
        // are (re)detected here.
        let mut saved = settings::load();
        saved.runtime_tier = runtime_tier;
        saved.dll_dirs = dll_dirs;
        saved.runtime_setup_seen = true;
        settings::save(&saved)?;
        Ok(saved)
    })
    .await
}

/// Persist the watch launch options the user sets in the Settings panel. Every
/// click-to-watch in the picker launches with these.
#[tauri::command]
pub async fn set_watch_options(
    auto: bool,
    focus_only: bool,
    word_underlines: bool,
    hud: bool,
    metrics_interval_secs: u64,
    auto_eval_capture: bool,
    eval_capture_root: Option<std::path::PathBuf>,
) -> Result<AppSettings, String> {
    blocking(move || {
        let mut saved = settings::load();
        saved.watch_auto = auto;
        saved.watch_focus_only = focus_only;
        saved.watch_word_underlines = word_underlines;
        saved.watch_hud = hud;
        saved.watch_metrics_interval_secs = metrics_interval_secs;
        saved.auto_eval_capture = auto_eval_capture;
        saved.eval_capture_root = eval_capture_root;
        saved.eval_capture_root()?;
        settings::save(&saved)?;
        Ok(saved)
    })
    .await
}

/// The persisted app settings (recorded runtime tier, DLL dirs, seen flag).
#[tauri::command]
pub fn get_settings() -> AppSettings {
    settings::load()
}

/// Whether a `pip` executable is discoverable on `PATH`. The guided runtime
/// setup defaults to the pip install route when it is, since that route is a
/// single copy-paste command. Pure PATH inspection: it never spawns pip.
#[tauri::command]
pub fn pip_available() -> bool {
    let names: &[&str] = if cfg!(windows) {
        &["pip.exe", "pip3.exe"]
    } else {
        &["pip", "pip3"]
    };
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
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
pub fn start_watch(
    app: AppHandle,
    state: State<WatchState>,
    engine: State<EngineUse>,
    window_id: u32,
    window_title: String,
) -> Result<(), String> {
    watch::start(&app, &state, &engine, window_id, &window_title).map_err(|err| format!("{err:#}"))
}

/// Kick off the one-shot engine warmup (`mite warmup --json`). Progress arrives
/// as `warmup-event` / `warmup-state` events; a call while one is already
/// running joins it instead of spawning a second child.
#[tauri::command]
pub fn start_warmup(app: AppHandle, engine: State<EngineUse>) -> Result<(), String> {
    warmup::start(&app, &engine).map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub fn is_warming(engine: State<EngineUse>) -> bool {
    warmup::is_warming(&engine)
}

#[tauri::command]
pub fn stop_watch(state: State<WatchState>) {
    watch::stop(&state);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_ok_when_newest_compatible_is_installed() {
        assert_eq!(
            engine_state("v0.2.0", Some("0.2.0"), Some("0.2.0")),
            EngineState::Ok
        );
        // A newer-but-still-compatible target with current already there is Ok.
        assert_eq!(
            engine_state("v0.2.1", Some("0.2.1"), Some("0.2.1")),
            EngineState::Ok
        );
    }

    #[test]
    fn engine_update_when_newer_compatible_exists() {
        assert_eq!(
            engine_state("v0.2.0", Some("0.2.0"), Some("0.2.1")),
            EngineState::Update
        );
    }

    #[test]
    fn engine_required_when_installed_is_out_of_range() {
        // The classic case: the app self-updated to 0.2.0 but the engine is the
        // old 0.1.0. 0.1.0 is outside ^0.2.0, so reconciling is required.
        assert_eq!(
            engine_state("v0.2.0", Some("0.1.0"), Some("0.2.0")),
            EngineState::Required
        );
        // An engine ahead of the app's range is equally incompatible.
        assert_eq!(
            engine_state("v0.2.0", Some("0.3.0"), Some("0.2.0")),
            EngineState::Required
        );
    }

    #[test]
    fn engine_required_when_missing() {
        assert_eq!(
            engine_state("v0.2.0", None, Some("0.2.0")),
            EngineState::Required
        );
    }

    #[test]
    fn engine_unknown_when_target_unresolved() {
        assert_eq!(
            engine_state("v0.2.0", Some("0.2.0"), None),
            EngineState::Unknown
        );
    }

    #[test]
    fn placeholder_app_version_compares_against_target_only() {
        // Untagged local build (no caret pin): fall back to plain version compare
        // against the latest target.
        assert_eq!(
            engine_state("0.0.0", Some("0.1.0"), Some("0.2.0")),
            EngineState::Update
        );
        assert_eq!(
            engine_state("0.0.0", Some("0.2.0"), Some("0.2.0")),
            EngineState::Ok
        );
    }

    fn extra(asset: &str) -> release::AssetRef {
        release::AssetRef {
            asset: asset.to_string(),
            sha256: "x".to_string(),
        }
    }

    #[test]
    fn sidecars_present_requires_every_declared_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        let extras = vec![
            extra("onnxruntime_providers_shared.dll"),
            extra("onnxruntime_providers_cuda.dll"),
        ];

        // Nothing installed yet.
        assert!(!sidecars_present(bin, &extras));
        // One present, one missing is still incomplete: this is the interrupted
        // install / pre-sidecar upgrade case that the version check cannot see.
        std::fs::write(bin.join("onnxruntime_providers_shared.dll"), b"").unwrap();
        assert!(!sidecars_present(bin, &extras));
        // Both present is complete.
        std::fs::write(bin.join("onnxruntime_providers_cuda.dll"), b"").unwrap();
        assert!(sidecars_present(bin, &extras));
        // An older release with no declared sidecars is trivially complete.
        assert!(sidecars_present(bin, &[]));
    }
}
