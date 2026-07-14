//! Supervising the one-shot `mite warmup --json` engine preparation.
//!
//! The first engine build after an install, update, or GPU-tier change
//! compiles TensorRT engines, which takes minutes and used to happen silently
//! inside the first watch. The frontend runs warmup instead (at launch and
//! after anything that could invalidate the engines), renders its progress,
//! and keeps the Watch tab gated until it finishes. The child is built by
//! `cli::command()`, so it runs in the mite home with the recorded NVIDIA
//! runtime on `PATH` and the recorded tier as `--backend`: it prepares exactly
//! the sessions `watch` will use.
//!
//! Each stdout line is one JSON event (see the CLI's `WarmupEvent`); they are
//! forwarded to the frontend verbatim as `warmup-event`. Start and exit are
//! reported as `warmup-state`, with a stderr tail attached on failure. stderr
//! is also mirrored to `logs\warmup-<pid>.log` for diagnosis.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::engine_use::{EngineUse, EngineUser};
use crate::{cli, home};

/// How many trailing stderr lines to keep in memory for the failure report.
const STDERR_TAIL_LINES: usize = 12;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WarmupStateEvent {
    running: bool,
    code: Option<i32>,
    /// The stderr tail when the child exited nonzero, for the error banner.
    error: Option<String>,
}

/// True when a warmup child currently holds the engine claim.
pub fn is_warming(engine: &EngineUse) -> bool {
    engine.current() == EngineUser::Warming
}

/// Accept a stdout line only if it is a JSON object carrying an `event` tag;
/// anything else (stray prints, partial lines) is dropped rather than crashing
/// the frontend's event handling.
fn parse_warmup_line(line: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    value.get("event")?.as_str()?;
    Some(value)
}

/// Start `mite warmup --json` and stream its progress to the frontend. A
/// second call while one is already running is a no-op: the caller's event
/// listeners pick up the in-flight run.
pub fn start(app: &AppHandle, engine: &State<EngineUse>) -> Result<()> {
    // Claiming the engine before anything else makes warmup-vs-watch mutual
    // exclusion atomic: both compile into the same engine cache, so exactly one
    // may run. A claim already held by another warmup means one is in flight
    // and the caller's event listeners will pick it up.
    match engine.try_claim(EngineUser::Warming) {
        Ok(()) => {}
        Err(EngineUser::Warming) => return Ok(()),
        Err(_) => anyhow::bail!("watch is running; the engines are already in use"),
    }

    if let Err(error) = spawn_supervised(app, engine) {
        engine.release(EngineUser::Warming);
        return Err(error);
    }
    Ok(())
}

/// Spawn the warmup child and its pump/reaper threads. The caller has already
/// claimed the engine; on success the reaper thread owns releasing the claim,
/// on error the caller releases it.
fn spawn_supervised(app: &AppHandle, engine: &State<EngineUse>) -> Result<()> {
    if !home::cli_exe()?.exists() {
        anyhow::bail!("the mite CLI is not installed yet");
    }

    let mut cmd = cli::command()?;
    cmd.arg("warmup").arg("--json");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to launch mite warmup")?;
    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("missing warmup pipes");
        }
    };

    let _ = app.emit(
        "warmup-state",
        WarmupStateEvent {
            running: true,
            code: None,
            error: None,
        },
    );

    // stdout: one JSON event per line, forwarded verbatim.
    let event_app = app.clone();
    let out_handle = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(event) = parse_warmup_line(&line) {
                let _ = event_app.emit("warmup-event", event);
            }
        }
    });

    // stderr: mirror to a log file and keep a short tail for the error report.
    let log = open_log_file(child.id());
    let tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tail_writer = tail.clone();
    let err_handle = std::thread::spawn(move || {
        let mut log = log;
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(file) = log.as_mut() {
                let _ = writeln!(file, "{line}");
            }
            if let Ok(mut tail) = tail_writer.lock() {
                tail.push(line);
                if tail.len() > STDERR_TAIL_LINES {
                    tail.remove(0);
                }
            }
        }
    });

    // Reaper: once both pipes close, reap the child, release the engine claim,
    // and report the outcome (with the stderr tail on failure).
    let claim = engine.inner().clone();
    let reaper_app = app.clone();
    std::thread::spawn(move || {
        let _ = out_handle.join();
        let _ = err_handle.join();
        let code = child.wait().ok().and_then(|status| status.code());
        claim.release(EngineUser::Warming);
        let error = match code {
            Some(0) => None,
            _ => Some(
                tail.lock()
                    .map(|lines| lines.join("\n"))
                    .unwrap_or_default(),
            ),
        };
        let _ = reaper_app.emit(
            "warmup-state",
            WarmupStateEvent {
                running: false,
                code,
                error,
            },
        );
    });

    Ok(())
}

fn open_log_file(pid: u32) -> Option<File> {
    let dir = home::logs_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    File::create(dir.join(format!("warmup-{pid}.log"))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_tagged_json_objects() {
        let event = parse_warmup_line(r#"{"event":"start","targets":["detector"]}"#)
            .expect("tagged object parses");
        assert_eq!(event["event"], "start");

        // Whitespace around the line is tolerated (Windows \r survives lines()).
        assert!(parse_warmup_line(" {\"event\":\"done\",\"elapsedMs\":1}\r").is_some());

        // Non-JSON, non-object, and untagged lines are dropped.
        assert!(parse_warmup_line("preparing 2 OCR session(s)").is_none());
        assert!(parse_warmup_line("[1,2,3]").is_none());
        assert!(parse_warmup_line(r#"{"other":"field"}"#).is_none());
        assert!(parse_warmup_line(r#"{"event":42}"#).is_none());
    }
}
