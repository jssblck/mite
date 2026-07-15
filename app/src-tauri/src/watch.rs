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

use crate::engine_use::{EngineUse, EngineUser};
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
/// Saved app settings provide the watch flags and capture destination; the
/// picker only supplies the window to read and its title.
pub fn start(
    app: &AppHandle,
    state: &State<WatchState>,
    engine: &State<EngineUse>,
    window_id: u32,
    window_title: &str,
) -> Result<()> {
    // Claim the engine atomically: watch and warmup both compile into the same
    // TensorRT engine cache, so exactly one may run. The frontend gates the
    // Watch tab during warmup; this backstops commands racing past that gate.
    match engine.try_claim(EngineUser::Watching) {
        Ok(()) => {}
        Err(EngineUser::Warming) => {
            anyhow::bail!("the reading engine is still being prepared; try again in a moment")
        }
        Err(_) => anyhow::bail!("watch is already running"),
    }

    if let Err(error) = spawn_supervised(app, state, engine, window_id, window_title) {
        engine.release(EngineUser::Watching);
        return Err(error);
    }
    Ok(())
}

/// Spawn the watch child and its pump/reaper threads. The caller has already
/// claimed the engine; on success the reaper thread owns releasing the claim,
/// on error the caller releases it.
fn spawn_supervised(
    app: &AppHandle,
    state: &State<WatchState>,
    engine: &State<EngineUse>,
    window_id: u32,
    window_title: &str,
) -> Result<()> {
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
    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("missing watch pipes");
        }
    };

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

    // Reaper: once both pipes close (the process has exited), reap it, clear
    // the slot and the engine claim so a fresh start is allowed, and report
    // the exit code.
    let slot = state.0.clone();
    let claim = engine.inner().clone();
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
        claim.release(EngineUser::Watching);
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
    // Valid here because the app always pins a target with --window-id.
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
            watch_focus_only: false,
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

        assert_eq!(
            args(&cmd),
            vec!["--auto".to_string(), "--focus-only".to_string()]
        );
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
