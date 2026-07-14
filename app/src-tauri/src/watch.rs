//! Launching and supervising the long-running `mite watch` process.
//!
//! The child is built by `cli::command()`, which sets the mite home as the
//! working directory, prepends the recorded NVIDIA runtime directories to `PATH`
//! so the OS loader can resolve the TensorRT/CUDA DLLs the user installed, and
//! passes the recorded tier as `--backend`. stdout/stderr are streamed to the
//! UI as `watch-log` events and mirrored to a per-run log file; process
//! start/exit is reported as `watch-state`.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{cli, home, settings};

/// The single supervised `watch` child, shared with the reaper thread.
#[derive(Default)]
pub struct WatchState(pub Arc<Mutex<Option<Child>>>);

#[derive(Debug, Clone, Serialize)]
struct LogLine {
    line: String,
    stream: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchStateEvent {
    running: bool,
    code: Option<i32>,
}

/// True when a watch process is currently supervised.
pub fn is_watching(state: &State<WatchState>) -> bool {
    state.0.lock().map(|guard| guard.is_some()).unwrap_or(false)
}

/// Start `mite watch` for the given window. Errors if one is already running.
///
/// The watch flags (continuous mode, focus gating, HUD, metrics interval) come
/// from the saved app settings, which the user configures in the Settings
/// panel; the picker only supplies the window to read.
pub fn start(app: &AppHandle, state: &State<WatchState>, window_id: u32) -> Result<()> {
    if is_watching(state) {
        anyhow::bail!("watch is already running");
    }

    if !home::cli_exe()?.exists() {
        anyhow::bail!("the mite CLI is not installed yet");
    }

    let opts = settings::load();

    // cli::command() applies the home CWD, the NVIDIA runtime PATH, the
    // CREATE_NO_WINDOW flag, and the recorded `--backend` override.
    let mut cmd = cli::command()?;
    cmd.arg("watch")
        .arg("--window-id")
        .arg(window_id.to_string());
    if opts.watch_auto {
        cmd.arg("--auto");
    }
    // Valid here unconditionally: --focus-only requires a pinned target, and
    // the app always pins with --window-id above.
    if opts.watch_focus_only {
        cmd.arg("--focus-only");
    }
    if opts.watch_hud {
        cmd.arg("--hud");
    }
    if opts.watch_metrics_interval_secs > 0 {
        cmd.arg("--metrics-interval-secs")
            .arg(opts.watch_metrics_interval_secs.to_string());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to launch mite watch")?;
    let stdout = child.stdout.take().context("missing stdout pipe")?;
    let stderr = child.stderr.take().context("missing stderr pipe")?;

    let log = open_log_file(child.id()).map(|file| Arc::new(Mutex::new(file)));
    let out_handle = pump(app.clone(), stdout, "stdout", log.clone());
    let err_handle = pump(app.clone(), stderr, "stderr", log.clone());

    *state.0.lock().unwrap() = Some(child);
    let _ = app.emit(
        "watch-state",
        WatchStateEvent {
            running: true,
            code: None,
        },
    );

    // Reaper: once both pipes close (the process has exited), reap it, report
    // the exit code, and clear the slot so a fresh start is allowed.
    let slot = state.0.clone();
    let reaper_app = app.clone();
    std::thread::spawn(move || {
        let _ = out_handle.join();
        let _ = err_handle.join();
        let code = slot
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
            .and_then(|mut child| child.wait().ok())
            .and_then(|status| status.code());
        let _ = reaper_app.emit(
            "watch-state",
            WatchStateEvent {
                running: false,
                code,
            },
        );
    });

    Ok(())
}

/// Kill the supervised watch process if one is running. The reaper thread then
/// reaps it and emits the final `watch-state`.
pub fn stop(state: &State<WatchState>) {
    if let Some(child) = state.0.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
}

fn open_log_file(pid: u32) -> Option<File> {
    let dir = home::logs_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    File::create(dir.join(format!("watch-{pid}.log"))).ok()
}

/// Spawn a thread that reads a pipe line by line, emits each line as a
/// `watch-log` event, and mirrors it to the log file.
///
/// The UI receives the line verbatim (its log panel parses and renders the
/// ANSI colors the CLI's tracing output carries); the log file gets the line
/// with escape sequences stripped so it stays grep-able plain text.
fn pump(
    app: AppHandle,
    reader: impl std::io::Read + Send + 'static,
    stream: &'static str,
    log: Option<Arc<Mutex<File>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(log) = &log {
                if let Ok(mut file) = log.lock() {
                    let _ = writeln!(file, "[{stream}] {}", strip_ansi(&line));
                }
            }
            let _ = app.emit("watch-log", LogLine { line, stream });
        }
    })
}

/// Remove ANSI escape sequences (CSI, OSC, and other escapes) from a line,
/// keeping only the visible text. The vte terminal parser underneath
/// strip-ansi-escapes handles the escape tokenization.
fn strip_ansi(line: &str) -> String {
    strip_ansi_escapes::strip_str(line)
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn passes_plain_text_through() {
        assert_eq!(strip_ansi("model warmup complete"), "model warmup complete");
    }

    #[test]
    fn strips_tracing_sgr_sequences() {
        let line = "\u{1b}[2m2026-07-14T00:33:54.845834Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \
                    \u{1b}[2mort::logging\u{1b}[0m\u{1b}[2m:\u{1b}[0m timing cache miss";
        assert_eq!(
            strip_ansi(line),
            "2026-07-14T00:33:54.845834Z  WARN ort::logging: timing cache miss"
        );
    }

    #[test]
    fn strips_non_sgr_csi_and_osc_sequences() {
        assert_eq!(
            strip_ansi("\u{1b}[2Kcleared \u{1b}]0;title\u{7}done \u{1b}]8;;x\u{1b}\\link"),
            "cleared done link"
        );
    }

    #[test]
    fn drops_truncated_escape_at_end_of_line() {
        assert_eq!(strip_ansi("done\u{1b}[3"), "done");
        assert_eq!(strip_ansi("done\u{1b}"), "done");
    }

    #[test]
    fn keeps_multibyte_text_intact() {
        assert_eq!(strip_ansi("\u{1b}[32m見て\u{1b}[0m"), "見て");
    }
}
