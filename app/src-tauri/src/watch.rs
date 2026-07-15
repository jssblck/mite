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
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{cli, eval_capture, home, settings};

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
/// The watch flags (continuous mode, HUD, metrics interval) come from the saved
/// app settings, which the user configures in the Settings panel; the picker
/// only supplies the window to read.
pub fn start(
    app: &AppHandle,
    state: &State<WatchState>,
    window_id: u32,
    window_title: &str,
) -> Result<()> {
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
    apply_watch_options(&mut cmd, &opts, window_id, window_title)?;
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

/// Apply every persisted watch option and prepare any required output path.
///
/// Eval capture is preflighted before the child starts so an invalid or
/// unwritable saved root fails at launch instead of losing scenes later.
fn apply_watch_options(
    cmd: &mut Command,
    opts: &settings::AppSettings,
    window_id: u32,
    window_title: &str,
) -> Result<()> {
    if opts.watch_auto {
        cmd.arg("--auto");
    }
    if opts.watch_hud {
        cmd.arg("--hud");
    }
    if opts.watch_metrics_interval_secs > 0 {
        cmd.arg("--metrics-interval-secs")
            .arg(opts.watch_metrics_interval_secs.to_string());
    }
    if let Some(root) = opts.eval_capture_root()? {
        let output_dir = eval_capture::output_dir(root, window_title, window_id);
        prepare_capture_dir(&output_dir)?;
        cmd.arg("--auto-eval-capture")
            .arg("--eval-capture-dir")
            .arg(output_dir);
    }
    Ok(())
}

fn prepare_capture_dir(output_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("preparing eval capture directory {}", output_dir.display()))?;

    for suffix in 0..16 {
        let probe = output_dir.join(format!(".mite-write-test-{}-{suffix}", std::process::id()));
        match std::fs::create_dir(&probe) {
            Ok(()) => {
                let probe_file = probe.join("write-test");
                if let Err(error) = std::fs::write(&probe_file, b"") {
                    let _ = std::fs::remove_dir(&probe);
                    return Err(error).with_context(|| {
                        format!(
                            "testing file writes under eval capture directory {}",
                            output_dir.display()
                        )
                    });
                }
                std::fs::remove_file(&probe_file).with_context(|| {
                    format!(
                        "cleaning up eval capture write probe {}",
                        probe_file.display()
                    )
                })?;
                std::fs::remove_dir(&probe).with_context(|| {
                    format!(
                        "cleaning up eval capture directory probe {}",
                        probe.display()
                    )
                })?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "testing directory creation under eval capture directory {}",
                        output_dir.display()
                    )
                });
            }
        }
    }

    anyhow::bail!(
        "could not allocate a write probe under eval capture directory {}",
        output_dir.display()
    )
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
                    let _ = writeln!(file, "[{stream}] {line}");
                }
            }
            let _ = app.emit("watch-log", LogLine { line, stream });
        }
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(OsStr::to_string_lossy)
            .map(|arg| arg.into_owned())
            .collect()
    }

    #[test]
    fn enabled_eval_capture_prepares_and_passes_the_normalized_output_dir() {
        let root = tempfile::tempdir().unwrap();
        let opts = settings::AppSettings {
            watch_auto: false,
            auto_eval_capture: true,
            eval_capture_root: Some(root.path().to_path_buf()),
            ..settings::AppSettings::default()
        };
        let expected = root.path().join("grace-s-game");
        let mut cmd = Command::new("mite");

        apply_watch_options(&mut cmd, &opts, 42, "Grace's Game").unwrap();

        assert!(expected.is_dir());
        assert_eq!(std::fs::read_dir(&expected).unwrap().count(), 0);
        assert_eq!(
            args(&cmd),
            vec![
                "--auto-eval-capture".to_string(),
                "--eval-capture-dir".to_string(),
                expected.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn disabled_eval_capture_omits_capture_arguments() {
        let opts = settings::AppSettings::default();
        let mut cmd = Command::new("mite");

        apply_watch_options(&mut cmd, &opts, 42, "Grace's Game").unwrap();

        assert_eq!(args(&cmd), vec!["--auto".to_string()]);
    }

    #[test]
    fn enabled_eval_capture_rejects_a_root_replaced_by_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("capture-root");
        std::fs::write(&root, b"not a directory").unwrap();
        let opts = settings::AppSettings {
            auto_eval_capture: true,
            eval_capture_root: Some(root),
            ..settings::AppSettings::default()
        };
        let mut cmd = Command::new("mite");

        let error = apply_watch_options(&mut cmd, &opts, 42, "Grace's Game").unwrap_err();

        assert!(error
            .to_string()
            .contains("preparing eval capture directory"));
    }
}
