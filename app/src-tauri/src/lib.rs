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
mod window;
mod windows;

use watch::WatchState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    // Self-update: the updater downloads and verifies the next signed installer,
    // and the process plugin lets the frontend relaunch into it. The window-state
    // plugin remembers the main window's size and position across launches. All
    // three are desktop-only, which matches this Windows-only app.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_window_state::Builder::default().build());
    }

    builder
        .setup(|app| {
            // On the first launch there is no saved window state to restore, so
            // size the window to the screen instead of the small config default.
            #[cfg(desktop)]
            apply_first_run_window_size(app);
            Ok(())
        })
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
            commands::set_watch_options,
            commands::pip_available,
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

/// Size and center the main window on first launch only.
///
/// The window-state plugin restores the saved size and position whenever its
/// state file exists, so we defer to it on every later launch and only impose a
/// screen-relative default when that file is absent.
#[cfg(desktop)]
fn apply_first_run_window_size(app: &tauri::App) {
    use tauri::Manager;

    let restored = app
        .path()
        .app_config_dir()
        .map(|dir| {
            dir.join(tauri_plugin_window_state::DEFAULT_FILENAME)
                .exists()
        })
        .unwrap_or(false);
    if restored {
        return;
    }

    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let Ok(Some(monitor)) = main.current_monitor() else {
        return;
    };
    let screen = monitor.size().to_logical::<f64>(monitor.scale_factor());
    let (width, height) = window::pick_window_size(screen.width, screen.height);
    let _ = main.set_size(tauri::LogicalSize::new(width, height));
    let _ = main.center();
}
