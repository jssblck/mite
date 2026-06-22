//! Launching and supervising the long-running `mite watch` process.
//!
//! The child is spawned with the mite home as its working directory and the GPU
//! runtime dir both exported (`MITE_GPU_RUNTIME_DIR`) and prepended to `PATH`
//! so the OS loader can resolve the TensorRT/CUDA DLLs. stdout/stderr are
//! streamed to the UI as `watch-log` events and mirrored to a per-run log file;
//! process start/exit is reported as `watch-state`.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::home;

/// The single supervised `watch` child, shared with the reaper thread.
#[derive(Default)]
pub struct WatchState(pub Arc<Mutex<Option<Child>>>);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchOptions {
    pub window_id: u32,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub hud: bool,
    #[serde(default)]
    pub metrics_interval_secs: u64,
}

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
pub fn start(app: &AppHandle, state: &State<WatchState>, opts: WatchOptions) -> Result<()> {
    if is_watching(state) {
        anyhow::bail!("watch is already running");
    }

    let exe = home::cli_exe()?;
    if !exe.exists() {
        anyhow::bail!("the mite CLI is not installed yet");
    }
    let home_dir = home::mite_home()?;
    let gpu = home::gpu_runtime_dir()?;
    let gpu_str = gpu.to_string_lossy().to_string();
    let path = std::env::var("PATH").unwrap_or_default();
    let new_path = if path.is_empty() {
        gpu_str.clone()
    } else {
        format!("{gpu_str};{path}")
    };

    let mut cmd = Command::new(&exe);
    cmd.arg("watch")
        .arg("--window-id")
        .arg(opts.window_id.to_string());
    if opts.auto {
        cmd.arg("--auto");
    }
    if opts.hud {
        cmd.arg("--hud");
    }
    if opts.metrics_interval_secs > 0 {
        cmd.arg("--metrics-interval-secs")
            .arg(opts.metrics_interval_secs.to_string());
    }
    cmd.current_dir(&home_dir)
        .env("MITE_GPU_RUNTIME_DIR", &gpu)
        .env("PATH", new_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

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
pub fn stop(state: &State<WatchState>) -> Result<()> {
    if let Some(child) = state.0.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    Ok(())
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
