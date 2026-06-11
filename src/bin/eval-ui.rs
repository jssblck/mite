use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use mite::eval_ui::{EvalUiOptions, run_eval_ui};

#[derive(Debug, Parser)]
#[command(name = "eval-ui")]
#[command(about = "Local developer UI for reviewing and authoring Mite eval labels.")]
struct Cli {
    /// Mite config file. Built-in defaults are used when the file is absent.
    #[arg(short, long, default_value = "mite.toml")]
    config: PathBuf,
    /// Eval root shown by the UI.
    #[arg(long, default_value = "eval")]
    eval_root: PathBuf,
    /// JMdict lexicon used to synthesize token metadata for new labels.
    #[arg(long, default_value = "models/jmdict-eng.json")]
    lexicon: PathBuf,
    /// Host interface for the local UI server.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port for the local UI server. Use 0 to ask the OS for a free port.
    #[arg(long, default_value_t = 8765)]
    port: u16,
    /// Use the built-in fixture OCR engine for UI development and smoke tests.
    #[arg(long)]
    fixture_ocr: bool,
    /// Minimum IoU for strict eval matching; tolerant bounds jitter can also match.
    #[arg(long, default_value_t = 0.50)]
    min_iou: f32,
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    run_eval_ui(EvalUiOptions {
        config_path: cli.config,
        eval_root: cli.eval_root,
        lexicon: cli.lexicon,
        host: cli.host,
        port: cli.port,
        fixture_ocr: cli.fixture_ocr,
        min_iou: cli.min_iou,
    })
}

fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer, fmt};

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,xcap=off,ort=warn"));
    tracing_subscriber::registry()
        .with(mite::hud::StageTimingLayer::default())
        .with(fmt::layer().with_filter(env_filter))
        .init();
}
