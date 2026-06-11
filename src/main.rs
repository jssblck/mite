use anyhow::Result;

fn main() -> Result<()> {
    init_tracing();
    mite::cli::run()
}

fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer, fmt};

    // xcap logs benign per-window enumeration failures (access-denied on system
    // processes) at ERROR, and ONNX Runtime is extremely chatty at INFO; quiet
    // both by default. Override with RUST_LOG.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,xcap=off,ort=warn"));
    tracing_subscriber::registry()
        // The stage-timing layer is intentionally unfiltered so the watch HUD's
        // per-stage timings don't depend on RUST_LOG verbosity. The log filter is
        // attached only to the fmt layer below.
        .with(mite::hud::StageTimingLayer::default())
        .with(fmt::layer().with_filter(env_filter))
        .init();
}
