//! Mite desktop app backend.
//!
//! Manages a per-user "mite home" directory: installs and updates the mite CLI
//! from the GitHub release feed, downloads the model/dictionary deps and the
//! optional GPU acceleration pack, runs diagnostics, drives a live window
//! picker, and launches/supervises `mite watch`.

mod cli;
mod commands;
mod download;
mod home;
mod models;
mod release;
mod status;
mod watch;
mod windows;

use watch::WatchState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(WatchState::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            commands::mite_home_path,
            commands::get_status,
            commands::check_for_updates,
            commands::install_or_update_cli,
            commands::download_models,
            commands::download_gpu_pack,
            commands::write_default_config,
            commands::list_windows,
            commands::capture_thumbnail,
            commands::start_watch,
            commands::stop_watch,
            commands::is_watching,
            commands::open_mite_home,
            commands::uninstall_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mite app");
}
