use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::capture::{WindowCapturePreference, WindowSelector, list_capturable_windows};
use crate::config::AppConfig;
use crate::doctor::DoctorReport;
use crate::{artifact, eval, eval_capture, hotkey, interactive, storage_cleanup};

#[derive(Debug, Parser)]
#[command(name = "mite")]
#[command(about = "Local-first low-latency OCR overlay pipeline for Windows/NVIDIA systems.")]
struct Cli {
    #[arg(short, long, default_value = "mite.toml")]
    config: PathBuf,

    /// Run the INT8-quantized detector and recognizer (overrides the
    /// `runtime.int8_*` config flags; see scripts/quantize-models.py).
    #[arg(long, global = true)]
    int8: bool,

    /// Run only the detector in INT8 (the recognizer keeps its configured
    /// precision).
    #[arg(long, global = true)]
    int8_detector: bool,

    /// Run only the recognizer in INT8 (the detector keeps its configured
    /// precision).
    #[arg(long, global = true)]
    int8_recognizer: bool,

    #[command(subcommand)]
    command: Command,
}

impl Cli {
    fn int8_override(&self) -> Int8Override {
        Int8Override {
            detector: self.int8 || self.int8_detector,
            recognizer: self.int8 || self.int8_recognizer,
        }
    }
}

/// INT8 selections from the CLI, applied on top of the loaded config.
#[derive(Debug, Clone, Copy)]
struct Int8Override {
    detector: bool,
    recognizer: bool,
}

/// Shared window-targeting flags. The criteria combine (a window must satisfy
/// all that are given).
#[derive(Debug, Args)]
struct WindowArgs {
    /// Match a window whose title contains this substring.
    #[arg(long)]
    title: Option<String>,
    /// Match a window by exact window id (from `list-windows`).
    #[arg(long)]
    window_id: Option<u32>,
    /// Match a window by process id.
    #[arg(long)]
    pid: Option<u32>,
    /// Capture path: auto (WGC then screenshot), wgc, or screenshot.
    #[arg(long, value_enum, default_value_t = WindowCapturePreference::Auto)]
    capture_backend: WindowCapturePreference,
}

impl WindowArgs {
    /// A selector only if some criterion was given; `None` means "follow the
    /// foreground window" (used by `watch`).
    fn optional_selector(&self) -> Result<Option<WindowSelector>> {
        if self.title.is_none() && self.window_id.is_none() && self.pid.is_none() {
            return Ok(None);
        }
        WindowSelector::new(self.title.clone(), self.window_id, self.pid).map(Some)
    }
}

/// All `watch` flags.
#[derive(Debug, Args)]
struct WatchArgs {
    #[arg(long, default_value = "models/jmdict-eng.json")]
    lexicon: PathBuf,
    /// Minimum delay between OCR passes while active, in milliseconds.
    #[arg(long, default_value_t = 600)]
    refresh_ms: u64,
    #[arg(long, default_value_t = 3)]
    max_senses: usize,
    #[arg(long, default_value_t = 4)]
    max_glosses: usize,
    #[command(flatten)]
    window: WindowArgs,
    /// Run continuously without holding Shift, for games that intercept the
    /// Shift key while focused (pin the target with --window-id/--title).
    #[arg(long)]
    auto: bool,
    /// Draw a per-stage latency HUD (graph + p50/p95/p99) in the top-left.
    #[arg(long)]
    hud: bool,
    /// If > 0, log an aggregated per-stage timing report to stderr every N
    /// seconds (headless equivalent of --hud).
    #[arg(long, default_value_t = 0)]
    metrics_interval_secs: u64,
    /// Disable temporal smoothing (force a full pass each time).
    #[arg(long)]
    no_smoothing: bool,
    /// Developer fixture tool: register a global hotkey that saves the raw
    /// captured frame without OCR, e.g. Ctrl+Alt+F12.
    #[arg(long, value_name = "COMBO")]
    enable_eval_hotkey: Option<hotkey::HotkeyCombo>,
    /// Directory where --enable-eval-hotkey writes raw capture folders.
    #[arg(long, value_name = "DIR")]
    eval_capture_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a default `mite.toml`.
    InitConfig {
        #[arg(long)]
        force: bool,
    },
    /// Probe the GPU and model files and report readiness.
    Doctor,
    /// List capturable windows (id, pid, geometry, title).
    ListWindows,
    /// Score one full image against manual eval labels.
    Eval {
        #[arg(long)]
        image: PathBuf,
        #[arg(long, value_name = "EVAL_JSON")]
        labels: PathBuf,
        #[arg(long, default_value = "models/jmdict-eng.json")]
        lexicon: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Minimum IoU for strict matching; tolerant bounds jitter can also match.
        #[arg(long, default_value_t = 0.50)]
        min_iou: f32,
        /// Print/write the report but exit 0 even when the score is imperfect.
        #[arg(long)]
        allow_failures: bool,
    },
    /// Score every eval.json under a corpus root with one shared OCR engine.
    EvalCorpus {
        #[arg(long, default_value = "eval")]
        root: PathBuf,
        #[arg(long, default_value = "models/jmdict-eng.json")]
        lexicon: PathBuf,
        /// Write the corpus summary report to this path.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Directory for per-image eval reports.
        #[arg(long, default_value = "target/eval/corpus")]
        out_dir: PathBuf,
        /// Minimum IoU for strict matching; tolerant bounds jitter can also match.
        #[arg(long, default_value_t = 0.50)]
        min_iou: f32,
        /// Number of lowest-scoring image summaries to print.
        #[arg(long, default_value_t = 20)]
        worst: usize,
        /// Required aggregate score for a successful exit.
        #[arg(long, default_value_t = 1.0)]
        min_aggregate: f32,
        /// Print/write reports but exit 0 even when the aggregate is below threshold.
        #[arg(long)]
        allow_failures: bool,
    },
    /// Delete image files under `%LOCALAPPDATA%\mite`.
    CleanImages {
        /// Print the images that would be deleted without removing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Live point-and-define overlay.
    Watch(WatchArgs),
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let int8 = cli.int8_override();

    match cli.command {
        Command::InitConfig { force } => cmd_init_config(&cli.config, force),
        Command::Doctor => cmd_doctor(&cli.config, int8),
        Command::ListWindows => cmd_list_windows(),
        Command::Eval {
            image,
            labels,
            lexicon,
            out,
            min_iou,
            allow_failures,
        } => cmd_eval(EvalCommand {
            config_path: &cli.config,
            int8,
            image: &image,
            labels: &labels,
            lexicon: &lexicon,
            out,
            min_iou,
            allow_failures,
        }),
        Command::EvalCorpus {
            root,
            lexicon,
            out,
            out_dir,
            min_iou,
            worst,
            min_aggregate,
            allow_failures,
        } => cmd_eval_corpus(EvalCorpusCommand {
            config_path: &cli.config,
            int8,
            root: &root,
            lexicon: &lexicon,
            out,
            out_dir,
            min_iou,
            worst,
            min_aggregate,
            allow_failures,
        }),
        Command::CleanImages { dry_run } => cmd_clean_images(dry_run),
        Command::Watch(args) => cmd_watch(&cli.config, int8, args),
    }
}

fn cmd_init_config(config_path: &Path, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            config_path.display()
        );
    }
    AppConfig::write_default(config_path)?;
    println!("wrote {}", config_path.display());
    Ok(())
}

fn cmd_doctor(config_path: &Path, int8: Int8Override) -> Result<()> {
    let report = DoctorReport::inspect(&load_or_default(config_path, int8)?);
    print!("{}", report.render_text());
    Ok(())
}

fn cmd_list_windows() -> Result<()> {
    for window in list_capturable_windows()? {
        let label = if window.title.is_empty() {
            window.app_name
        } else {
            window.title
        };
        println!(
            "{} | pid={} | {}x{} @ {},{} | {}",
            window.id, window.pid, window.width, window.height, window.x, window.y, label
        );
    }
    Ok(())
}

struct EvalCommand<'a> {
    config_path: &'a Path,
    int8: Int8Override,
    image: &'a Path,
    labels: &'a Path,
    lexicon: &'a Path,
    out: Option<PathBuf>,
    min_iou: f32,
    allow_failures: bool,
}

fn cmd_eval(command: EvalCommand<'_>) -> Result<()> {
    if !(0.0..=1.0).contains(&command.min_iou) {
        bail!("--min-iou must be in [0, 1], got {}", command.min_iou);
    }
    let config = load_or_default(command.config_path, command.int8)?;
    let report = eval::run_eval(
        &config,
        command.image,
        command.labels,
        command.lexicon,
        eval::EvalOptions {
            min_iou: command.min_iou,
        },
    )?;
    eval::render_eval_report(&report);
    if let Some(out) = command.out {
        artifact::write_json_pretty(&out, &report)?;
        println!("wrote {}", out.display());
    }
    if !report.passed && !command.allow_failures {
        bail!(
            "eval score is imperfect: aggregate {:.1}%",
            report.aggregate_score * 100.0
        );
    }
    Ok(())
}

struct EvalCorpusCommand<'a> {
    config_path: &'a Path,
    int8: Int8Override,
    root: &'a Path,
    lexicon: &'a Path,
    out: Option<PathBuf>,
    out_dir: PathBuf,
    min_iou: f32,
    worst: usize,
    min_aggregate: f32,
    allow_failures: bool,
}

fn cmd_eval_corpus(command: EvalCorpusCommand<'_>) -> Result<()> {
    if !(0.0..=1.0).contains(&command.min_iou) {
        bail!("--min-iou must be in [0, 1], got {}", command.min_iou);
    }
    if !(0.0..=1.0).contains(&command.min_aggregate) {
        bail!(
            "--min-aggregate must be in [0, 1], got {}",
            command.min_aggregate
        );
    }
    let config = load_or_default(command.config_path, command.int8)?;
    let report = eval::run_eval_corpus(
        &config,
        command.root,
        command.lexicon,
        eval::EvalCorpusOptions {
            min_iou: command.min_iou,
            out_dir: Some(command.out_dir),
            progress: true,
        },
    )?;
    eval::render_eval_corpus_report(&report, command.worst);
    if let Some(out) = command.out {
        artifact::write_json_pretty(&out, &report)?;
        println!("wrote {}", out.display());
    }
    if report.aggregate_score + 0.0001 < command.min_aggregate && !command.allow_failures {
        bail!(
            "eval corpus aggregate {:.2}% is below required {:.2}%",
            report.aggregate_score * 100.0,
            command.min_aggregate * 100.0
        );
    }
    Ok(())
}

fn cmd_clean_images(dry_run: bool) -> Result<()> {
    let root = storage_cleanup::default_app_storage_root()?;
    let report = storage_cleanup::clean_app_images(&root, dry_run)?;
    let action = if dry_run { "would delete" } else { "deleted" };
    println!(
        "{action} {} image file(s) under {}",
        report.image_count(),
        report.root.display()
    );
    for image in &report.images {
        println!("{}", image.display());
    }
    Ok(())
}

fn cmd_watch(config_path: &Path, int8: Int8Override, args: WatchArgs) -> Result<()> {
    let config = load_or_default(config_path, int8)?;
    let backend = args.window.capture_backend;
    let eval_hotkey = match args.enable_eval_hotkey {
        Some(combo) => Some(interactive::EvalHotkeyRequest {
            combo,
            output_dir: args
                .eval_capture_dir
                .unwrap_or_else(eval_capture::default_capture_root),
        }),
        None => {
            if args.eval_capture_dir.is_some() {
                bail!("--eval-capture-dir requires --enable-eval-hotkey");
            }
            None
        }
    };
    // If any window selector is given, pin to a concrete window id so the loop
    // captures it regardless of which window is foreground.
    let pinned_window_id = match args.window.optional_selector()? {
        Some(selector) => Some(resolve_window_id(&selector)?),
        None => None,
    };
    let request = interactive::WatchRequest {
        lexicon: args.lexicon,
        refresh: Duration::from_millis(args.refresh_ms),
        max_senses: args.max_senses,
        max_glosses: args.max_glosses,
        pinned_window_id,
        backend,
        auto: args.auto,
        hud: args.hud,
        metrics_interval: Duration::from_secs(args.metrics_interval_secs),
        smoothing: !args.no_smoothing,
        eval_hotkey,
    };
    interactive::run_watch(&config, &request)
}

/// Resolve a window selector to a single concrete window id (used to pin
/// `watch` to one window). Errors if nothing matches.
fn resolve_window_id(selector: &WindowSelector) -> Result<u32> {
    let windows = list_capturable_windows()?;
    let mut matches = windows
        .into_iter()
        .filter(|window| selector.matches(Some(window.id), Some(window.pid), Some(&window.title)));
    let first = matches
        .next()
        .with_context(|| format!("no capturable window matched {}", selector.describe()))?;
    println!(
        "pinned watch target: window-id={} pid={} {:?} ({}x{})",
        first.id, first.pid, first.title, first.width, first.height
    );
    Ok(first.id)
}

fn load_or_default(path: &Path, int8: Int8Override) -> Result<AppConfig> {
    let mut config = if path.exists() {
        AppConfig::load(path)?
    } else {
        AppConfig::default()
    };
    if int8.detector {
        config.runtime.int8_detector = true;
    }
    if int8.recognizer {
        config.runtime.int8_recognizer = true;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_watch_flags_without_changing_cli_contract() {
        let cli = Cli::try_parse_from([
            "mite",
            "--config",
            "custom.toml",
            "--int8-detector",
            "watch",
            "--title",
            "Game",
            "--capture-backend",
            "wgc",
            "--auto",
            "--hud",
            "--metrics-interval-secs",
            "5",
            "--no-smoothing",
        ])
        .expect("watch args parse");

        assert_eq!(cli.config, PathBuf::from("custom.toml"));
        assert!(cli.int8_detector);
        assert!(!cli.int8_recognizer);
        let Command::Watch(args) = cli.command else {
            panic!("expected watch command");
        };
        assert_eq!(args.lexicon, PathBuf::from("models/jmdict-eng.json"));
        assert_eq!(args.window.title.as_deref(), Some("Game"));
        assert_eq!(
            args.window.capture_backend,
            WindowCapturePreference::WindowsGraphicsCapture
        );
        assert!(args.auto);
        assert!(args.hud);
        assert_eq!(args.metrics_interval_secs, 5);
        assert!(args.no_smoothing);
    }

    #[test]
    fn parses_eval_corpus_defaults_without_changing_cli_contract() {
        let cli = Cli::try_parse_from(["mite", "eval-corpus"]).expect("eval-corpus args parse");
        let Command::EvalCorpus {
            root,
            lexicon,
            out,
            out_dir,
            min_iou,
            worst,
            min_aggregate,
            allow_failures,
        } = cli.command
        else {
            panic!("expected eval-corpus command");
        };

        assert_eq!(root, PathBuf::from("eval"));
        assert_eq!(lexicon, PathBuf::from("models/jmdict-eng.json"));
        assert_eq!(out, None);
        assert_eq!(out_dir, PathBuf::from("target/eval/corpus"));
        assert_eq!(min_iou, 0.50);
        assert_eq!(worst, 20);
        assert_eq!(min_aggregate, 1.0);
        assert!(!allow_failures);
    }
}
