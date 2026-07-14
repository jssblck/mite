//! Engine warmup: build (or validate) the OCR sessions ahead of `watch`.
//!
//! The first session build after an install, update, or backend change can
//! compile TensorRT engines from scratch, which takes minutes and used to
//! happen silently inside `watch` startup. `run_warmup` performs exactly the
//! builds `watch` would perform for the configured backend (TensorRT, CUDA, or
//! CPU, with the same fallback chain) and then pushes one tiny inference
//! through each session so any lazily deferred compilation or kernel autotuning
//! happens now. Progress streams through a caller-supplied sink; the CLI
//! prints it as text or JSON lines, and the desktop app renders it as a
//! progress banner while keeping the Watch tab gated.
//!
//! Warmup is idempotent: with a primed engine cache it completes in seconds.

use std::collections::BTreeMap;
use std::time::Instant;

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use serde::Serialize;

use crate::config::{ModelConfig, RuntimeBackend, RuntimeConfig};

use super::{
    DET_CHANNELS, DET_PROFILE_MIN_SIDE, EngineBuildEvent, EngineTarget, OrtOcrEngine, REC_CHANNELS,
    REC_INPUT_HEIGHT, REC_MIN_WIDTH,
};

/// One warmup progress event. Serialized as a single JSON object per line by
/// `mite warmup --json`; the desktop app forwards each line to its frontend
/// verbatim, so the field names are camelCase and the `event` tags are stable
/// API.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WarmupEvent {
    /// Warmup began. `targets` lists the sessions that will be prepared, in
    /// order, so a consumer can render "step N of M".
    Start {
        backend: RuntimeBackend,
        targets: Vec<&'static str>,
    },
    /// A session build began. `likelyCompile` is true when no cached TensorRT
    /// engine exists for this model, i.e. the multi-minute case.
    BuildStarted {
        target: &'static str,
        likely_compile: bool,
    },
    /// A session build finished on the given execution provider.
    BuildFinished {
        target: &'static str,
        provider: &'static str,
        elapsed_ms: u64,
    },
    /// A tiny synthetic inference began, forcing any deferred compilation and
    /// kernel autotuning to happen now instead of on the first real frame.
    WarmStarted { target: &'static str },
    WarmFinished {
        target: &'static str,
        elapsed_ms: u64,
    },
    /// Warmup completed; `providers` maps each target to the execution
    /// provider it landed on.
    Done {
        elapsed_ms: u64,
        providers: BTreeMap<&'static str, &'static str>,
    },
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Build every session `watch` would build for this config and warm each with
/// one synthetic inference, streaming progress through `emit`.
pub fn run_warmup(
    models: &ModelConfig,
    runtime: &RuntimeConfig,
    emit: &mut dyn FnMut(WarmupEvent),
) -> Result<()> {
    let started = Instant::now();

    // The fixture backend has no real sessions; report an empty, instant run so
    // consumers see the same event shape everywhere.
    if matches!(runtime.backend, RuntimeBackend::Fixture) {
        emit(WarmupEvent::Start {
            backend: runtime.backend,
            targets: Vec::new(),
        });
        emit(WarmupEvent::Done {
            elapsed_ms: elapsed_ms(started),
            providers: BTreeMap::new(),
        });
        return Ok(());
    }

    let mut targets = vec![
        EngineTarget::Detector.name(),
        EngineTarget::Recognizer.name(),
    ];
    if models.fallback_recognizer_path.is_some() {
        targets.push(EngineTarget::FallbackRecognizer.name());
    }
    emit(WarmupEvent::Start {
        backend: runtime.backend,
        targets,
    });

    let mut providers: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    let mut engine = {
        let mut observer = |event: EngineBuildEvent| match event {
            EngineBuildEvent::BuildStarted {
                target,
                likely_compile,
            } => emit(WarmupEvent::BuildStarted {
                target: target.name(),
                likely_compile,
            }),
            EngineBuildEvent::BuildFinished {
                target,
                provider,
                elapsed,
            } => {
                providers.insert(target.name(), provider.name());
                emit(WarmupEvent::BuildFinished {
                    target: target.name(),
                    provider: provider.name(),
                    elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                });
            }
        };
        OrtOcrEngine::new_with_observer(models, runtime, &mut observer)?
    };

    warm_session(
        &mut engine.detector,
        EngineTarget::Detector,
        detector_warm_shape(),
        emit,
    )?;
    warm_session(
        &mut engine.recognizer,
        EngineTarget::Recognizer,
        recognizer_warm_shape(),
        emit,
    )?;
    if let Some(fallback) = engine.fallback_recognizer.as_mut() {
        warm_session(
            fallback,
            EngineTarget::FallbackRecognizer,
            recognizer_warm_shape(),
            emit,
        )?;
    }

    emit(WarmupEvent::Done {
        elapsed_ms: elapsed_ms(started),
        providers,
    });
    Ok(())
}

/// Smallest in-profile detector input: the TensorRT optimization profile floor,
/// which also satisfies the model's multiple-of-32 constraint.
fn detector_warm_shape() -> [usize; 4] {
    [
        1,
        DET_CHANNELS,
        DET_PROFILE_MIN_SIDE as usize,
        DET_PROFILE_MIN_SIDE as usize,
    ]
}

/// Smallest in-profile recognizer input (one minimum-width line crop).
fn recognizer_warm_shape() -> [usize; 4] {
    [
        1,
        REC_CHANNELS,
        REC_INPUT_HEIGHT as usize,
        REC_MIN_WIDTH as usize,
    ]
}

/// Run one zero-filled inference through `session`. The output is discarded;
/// the point is the side effects (deferred engine compilation, cuDNN/CUDA
/// kernel selection) so the first real frame pays none of them.
fn warm_session(
    session: &mut Session,
    target: EngineTarget,
    shape: [usize; 4],
    emit: &mut dyn FnMut(WarmupEvent),
) -> Result<()> {
    emit(WarmupEvent::WarmStarted {
        target: target.name(),
    });
    let started = Instant::now();
    let len = shape.iter().product();
    let input = Tensor::from_array((shape, vec![0.0f32; len].into_boxed_slice()))?;
    session
        .run(ort::inputs![input])
        .with_context(|| format!("warmup inference failed for the {}", target.name()))?;
    emit(WarmupEvent::WarmFinished {
        target: target.name(),
        elapsed_ms: elapsed_ms(started),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(event: &WarmupEvent) -> serde_json::Value {
        serde_json::to_value(event).expect("warmup events serialize")
    }

    #[test]
    fn events_serialize_with_stable_tags_and_camel_case_fields() {
        // The desktop app parses these lines; tag and field names are API.
        let start = json(&WarmupEvent::Start {
            backend: RuntimeBackend::NvidiaTensorRtThenCuda,
            targets: vec!["detector", "recognizer"],
        });
        assert_eq!(start["event"], "start");
        assert_eq!(start["backend"], "nvidia_tensor_rt_then_cuda");
        assert_eq!(start["targets"][0], "detector");

        let build = json(&WarmupEvent::BuildStarted {
            target: "detector",
            likely_compile: true,
        });
        assert_eq!(build["event"], "build_started");
        assert_eq!(build["likelyCompile"], true);

        let finished = json(&WarmupEvent::BuildFinished {
            target: "detector",
            provider: "tensor_rt",
            elapsed_ms: 1234,
        });
        assert_eq!(finished["event"], "build_finished");
        assert_eq!(finished["provider"], "tensor_rt");
        assert_eq!(finished["elapsedMs"], 1234);

        let done = json(&WarmupEvent::Done {
            elapsed_ms: 9,
            providers: BTreeMap::from([("detector", "cpu")]),
        });
        assert_eq!(done["event"], "done");
        assert_eq!(done["providers"]["detector"], "cpu");
    }

    #[test]
    fn fixture_backend_completes_instantly_with_no_targets() {
        let runtime = RuntimeConfig {
            backend: RuntimeBackend::Fixture,
            ..RuntimeConfig::default()
        };
        let mut events = Vec::new();
        run_warmup(&ModelConfig::default(), &runtime, &mut |event| {
            events.push(json(&event))
        })
        .expect("fixture warmup succeeds");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event"], "start");
        assert_eq!(events[0]["targets"].as_array().unwrap().len(), 0);
        assert_eq!(events[1]["event"], "done");
    }

    #[test]
    fn warm_shapes_stay_inside_the_tensorrt_profiles() {
        // The warm inference must never fall outside the optimization profile,
        // or TensorRT would silently fall back to CUDA for it.
        let [n, c, h, w] = detector_warm_shape();
        assert_eq!((n, c), (1, DET_CHANNELS));
        assert!(h as u32 >= DET_PROFILE_MIN_SIDE && w as u32 >= DET_PROFILE_MIN_SIDE);
        assert_eq!(h as u32 % 32, 0);
        assert_eq!(w as u32 % 32, 0);

        let [n, c, h, w] = recognizer_warm_shape();
        assert_eq!((n, c), (1, REC_CHANNELS));
        assert_eq!(h as u32, REC_INPUT_HEIGHT);
        assert!(w as u32 >= REC_MIN_WIDTH);
    }
}
