//! Mite desktop app backend.
//!
//! Manages a per-user "mite home" directory: installs and updates the mite CLI
//! from the GitHub release feed, downloads the model/dictionary deps, detects
//! and guides the user through installing NVIDIA's runtime (Mite never installs
//! those bytes itself), runs diagnostics, drives a live window picker, and
//! launches/supervises `mite watch`.

mod cli;
mod commands;
mod download;
mod home;
mod models;
mod release;
mod settings;
mod status;
mod watch;
mod windows;

use watch::WatchState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    // Self-update: the updater downloads and verifies the next signed installer,
    // and the process plugin lets the frontend relaunch into it. Both are
    // desktop-only, which matches this Windows-only app.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .manage(WatchState::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            commands::mite_home_path,
            commands::get_status,
            commands::check_for_updates,
            commands::install_or_update_cli,
            commands::download_models,
            commands::detect_runtime,
            commands::record_runtime,
            commands::get_settings,
            commands::write_default_config,
            commands::list_windows,
            commands::capture_thumbnail,
            commands::start_watch,
            commands::stop_watch,
            commands::is_watching,
            commands::open_mite_home,
            commands::open_url,
            commands::uninstall_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mite app");
}
