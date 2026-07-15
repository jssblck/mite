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
mod eval_capture;
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
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

    // Self-update: the updater downloads and verifies the next signed installer,
    // and the process plugin lets the frontend relaunch into it. The window-state
    // plugin remembers the main window's size and position across launches. All
    // three are desktop-only, which matches this Windows-only app.
    #[cfg(desktop)]
    {
        use tauri_plugin_window_state::StateFlags;

        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init())
            .plugin(
                tauri_plugin_window_state::Builder::default()
                    // Restore size and position, but never the visibility flag.
                    // The window is created hidden and revealed only after the
                    // frontend paints its first (already-dark) frame, which is
                    // what removes the blank white startup flash. Letting the
                    // plugin restore VISIBLE would re-show the blank window early
                    // on later launches and defeat that.
                    .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                    .build(),
            );
    }

    builder
        .setup(|app| {
            #[cfg(desktop)]
            {
                // On the first launch there is no saved window state to restore,
                // so size the window to the screen instead of the small config
                // default. This runs while the window is still hidden, so the
                // user never sees the resize.
                apply_first_run_window_size(app);
                // Safety net in case the frontend never asks to be shown.
                schedule_window_show_fallback(app);
            }
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

/// Reveal the main window after a grace period if the frontend never does.
///
/// The window is created hidden (`visible: false` in `tauri.conf.json`) and is
/// normally revealed by the frontend the moment it has painted its first frame
/// (see `app/src/main.tsx`). Painting first, showing second is what removes the
/// blank white default-position window Tauri would otherwise flash before the
/// dark UI mounts. If the frontend fails to load and never makes that call, this
/// fallback shows the window anyway so the user is never left staring at nothing.
#[cfg(desktop)]
fn schedule_window_show_fallback(app: &tauri::App) {
    use tauri::Manager;

    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        // Only force it visible if the frontend has not already done so, to avoid
        // stealing focus back from the user seconds after a normal startup.
        if !matches!(main.is_visible(), Ok(true)) {
            let _ = main.show();
            let _ = main.set_focus();
        }
    });
}
