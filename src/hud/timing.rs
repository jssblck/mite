use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tracing::span::Id;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::{PassTimings, Stage};

/// Lock-free latest-duration-per-stage sink. The [`StageTimingLayer`] writes the
/// most recent duration for each stage as its `tracing` span closes; the watch UI
/// reads all stages once per pass via [`StageClock::snapshot`] to assemble a
/// [`PassTimings`]. Stages run sequentially within a single in-flight pass, so
/// "latest per stage" is exactly the just-finished pass.
#[derive(Debug, Default)]
pub struct StageClock {
    capture_ns: AtomicU64,
    detect_ns: AtomicU64,
    recognize_ns: AtomicU64,
    analyze_ns: AtomicU64,
    present_ns: AtomicU64,
}

impl StageClock {
    /// Store the latest duration for a tracked stage (`Total` is ignored).
    pub fn record(&self, stage: Stage, elapsed: Duration) {
        let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
        let slot = match stage {
            Stage::Capture => &self.capture_ns,
            Stage::Detect => &self.detect_ns,
            Stage::Recognize => &self.recognize_ns,
            Stage::Analyze => &self.analyze_ns,
            Stage::Present => &self.present_ns,
            Stage::Total => return,
        };
        slot.store(ns, Ordering::Relaxed);
    }

    /// Assemble the current latest per-stage durations into one pass.
    pub fn snapshot(&self) -> PassTimings {
        let load = |slot: &AtomicU64| Duration::from_nanos(slot.load(Ordering::Relaxed));
        PassTimings {
            capture: load(&self.capture_ns),
            detect: load(&self.detect_ns),
            recognize: load(&self.recognize_ns),
            analyze: load(&self.analyze_ns),
            present: load(&self.present_ns),
        }
    }
}

/// Process-global stage clock, paired with the global `tracing` subscriber the
/// [`StageTimingLayer`] is installed into. Both the layer and the watch UI reach
/// it through here, so no `Arc` needs threading through call sites.
pub fn stage_clock() -> Arc<StageClock> {
    static STAGE_CLOCK: OnceLock<Arc<StageClock>> = OnceLock::new();
    STAGE_CLOCK
        .get_or_init(|| Arc::new(StageClock::default()))
        .clone()
}

/// Per-span start time, stashed in the span's extensions on enter.
struct StageStart(Instant);

/// A `tracing-subscriber` layer that times spans named after a pipeline stage
/// (`capture`, `detect`, ...) and feeds their durations to the global
/// [`stage_clock`]. Instrumenting a stage is then just wrapping it in a span;
/// nothing needs to return or thread timing values. Attach it without the log
/// `EnvFilter` so HUD timing is independent of `RUST_LOG` verbosity.
pub struct StageTimingLayer {
    clock: Arc<StageClock>,
}

impl Default for StageTimingLayer {
    fn default() -> Self {
        Self {
            clock: stage_clock(),
        }
    }
}

impl<S> Layer<S> for StageTimingLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && Stage::from_span_name(span.name()).is_some()
        {
            span.extensions_mut().replace(StageStart(Instant::now()));
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id)
            && let Some(stage) = Stage::from_span_name(span.name())
            && let Some(StageStart(start)) = span.extensions().get::<StageStart>()
        {
            self.clock.record(stage, start.elapsed());
        }
    }
}
