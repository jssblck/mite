use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ort::ep::CUDA;
use ort::ep::TensorRT;
use ort::ep::cuda::ConvAlgorithmSearch;
use ort::session::Session;

use crate::config::{RuntimeBackend, RuntimeConfig};

use super::{
    DET_CHANNELS, DET_PROFILE_MAX_SIDE, DET_PROFILE_MIN_SIDE, DET_PROFILE_OPT_HEIGHT,
    DET_PROFILE_OPT_WIDTH, EngineTarget, REC_BATCH_MAX, REC_CHANNELS, REC_INPUT_HEIGHT,
    REC_MAX_WIDTH, REC_MIN_WIDTH, REC_OPT_WIDTH,
};

pub(super) fn ensure_ort() {
    static INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let _ = INIT.get_or_init(|| ort::init().with_name("mite").commit());
}

/// Which model a session is for; selects the TensorRT optimization-profile shape
/// range (the detector takes one large image; the recognizer takes a batch of
/// fixed-height, variable-width line crops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelKind {
    Detector,
    Recognizer,
}

impl ModelKind {
    fn label(self) -> &'static str {
        match self {
            ModelKind::Detector => "detector",
            ModelKind::Recognizer => "recognizer",
        }
    }
}

/// The execution provider a committed session actually landed on after the
/// TensorRT -> CUDA -> CPU fallback chain resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveProvider {
    TensorRt,
    Cuda,
    Cpu,
}

impl ActiveProvider {
    /// Stable machine-facing name, matching the doctor report's tier strings.
    pub fn name(self) -> &'static str {
        match self {
            ActiveProvider::TensorRt => "tensor_rt",
            ActiveProvider::Cuda => "cuda",
            ActiveProvider::Cpu => "cpu",
        }
    }
}

/// The engine-cache file prefix for a session, shared by session setup and the
/// warmup cache probe so the two can never disagree about what "cached" means.
/// Keyed on the session target rather than the model kind: the primary and
/// fallback recognizers share a kind (and its TensorRT profile) but are
/// different models, so sharing a prefix would make one's cached engine mask
/// the other's cold-build probe.
pub(super) fn trt_cache_prefix(target: EngineTarget, int8: bool) -> String {
    let mut prefix = match target {
        EngineTarget::Detector => format!("mite-detector-max{DET_PROFILE_MAX_SIDE}"),
        EngineTarget::Recognizer => format!("mite-recognizer-max{REC_MAX_WIDTH}"),
        EngineTarget::FallbackRecognizer => {
            format!("mite-fallback-recognizer-max{REC_MAX_WIDTH}")
        }
    };
    if int8 {
        prefix.push_str("-int8");
    }
    prefix
}

/// Whether a serialized TensorRT engine for this model already sits in the
/// cache. The cache file name embeds a hash of the model and build options, so
/// this is a heuristic: a hit means a compile is *probably* not needed (TensorRT
/// still validates and rebuilds on a mismatch), while a miss means the first
/// session build will definitely compile from scratch, which takes minutes.
pub(super) fn trt_engine_cache_primed(cache_dir: &Path, prefix: &str) -> bool {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        name.starts_with(prefix) && name.ends_with(".engine")
    })
}

/// Whether committing a session for `target` under `runtime` is expected to
/// trigger a from-scratch TensorRT engine compile (the multi-minute case that
/// deserves a heads-up). CUDA and CPU sessions have no comparable build step.
pub(super) fn expects_trt_engine_build(runtime: &RuntimeConfig, target: EngineTarget) -> bool {
    if !matches!(runtime.backend, RuntimeBackend::NvidiaTensorRtThenCuda) {
        return false;
    }
    let prefix = trt_cache_prefix(target, runtime.int8_for(target == EngineTarget::Detector));
    !trt_engine_cache_primed(&runtime.engine_cache_dir, &prefix)
}

/// Build a session for `path`, preferring the fastest execution provider the
/// runtime asks for and degrading gracefully: TensorRT -> CUDA -> CPU. Each GPU
/// provider registers with `error_on_failure`, so a missing/incompatible runtime
/// surfaces as a logged warning and we fall back rather than silently running
/// slow with no explanation.
pub(super) fn commit_session(
    path: &Path,
    runtime: &RuntimeConfig,
    kind: ModelKind,
    target: EngineTarget,
) -> Result<(Session, ActiveProvider)> {
    ensure_ort();

    let label = kind.label();
    let try_trt = matches!(runtime.backend, RuntimeBackend::NvidiaTensorRtThenCuda);
    let try_cuda = matches!(
        runtime.backend,
        RuntimeBackend::NvidiaTensorRtThenCuda | RuntimeBackend::Cuda
    );

    if try_trt {
        match commit_trt_session(path, runtime, kind, target) {
            Ok(session) => {
                tracing::info!(
                    "{label}: TensorRT execution provider active ({})",
                    path.display()
                );
                return Ok((session, ActiveProvider::TensorRt));
            }
            Err(error) => {
                tracing::warn!(
                    "{label}: TensorRT EP unavailable ({error}); falling back to CUDA. \
                     Install the NVIDIA TensorRT/CUDA runtime and ensure it is on PATH, then run `mite doctor` to confirm the tier."
                );
            }
        }
    }

    if try_cuda {
        match commit_cuda_session(path) {
            Ok(session) => {
                tracing::info!(
                    "{label}: CUDA execution provider active ({})",
                    path.display()
                );
                return Ok((session, ActiveProvider::Cuda));
            }
            Err(error) => {
                tracing::warn!(
                    "{label}: CUDA execution provider unavailable ({error}); falling back to CPU. \
                     Install the NVIDIA CUDA/cuDNN runtime and ensure it is on PATH, then run `mite doctor` to confirm the tier."
                );
            }
        }
    }

    let session = Session::builder()?
        .commit_from_file(path)
        .with_context(|| format!("failed to load {label} model {}", path.display()))?;
    Ok((session, ActiveProvider::Cpu))
}

/// TensorRT optimization-profile shapes (`min`, `opt`, `max`) for a model's
/// dynamic input `x`. One engine built for this range serves every shape inside
/// it, so we never rebuild per line width - the key to making TensorRT usable for
/// the variable-width recognizer. Shapes are `NxCxHxW`.
pub(super) fn trt_profile_shapes(kind: ModelKind) -> (String, String, String) {
    match kind {
        ModelKind::Detector => (
            format!("x:1x{DET_CHANNELS}x{DET_PROFILE_MIN_SIDE}x{DET_PROFILE_MIN_SIDE}"),
            format!("x:1x{DET_CHANNELS}x{DET_PROFILE_OPT_HEIGHT}x{DET_PROFILE_OPT_WIDTH}"),
            format!("x:1x{DET_CHANNELS}x{DET_PROFILE_MAX_SIDE}x{DET_PROFILE_MAX_SIDE}"),
        ),
        ModelKind::Recognizer => (
            format!("x:1x{REC_CHANNELS}x{REC_INPUT_HEIGHT}x{REC_MIN_WIDTH}"),
            format!("x:{REC_BATCH_MAX}x{REC_CHANNELS}x{REC_INPUT_HEIGHT}x{REC_OPT_WIDTH}"),
            format!("x:{REC_BATCH_MAX}x{REC_CHANNELS}x{REC_INPUT_HEIGHT}x{REC_MAX_WIDTH}"),
        ),
    }
}

/// Build a session that requires the TensorRT EP to register, with FP16 enabled
/// and a single dynamic-shape optimization profile so the engine is built once
/// and reused for every input shape in range.
fn commit_trt_session(
    path: &Path,
    runtime: &RuntimeConfig,
    kind: ModelKind,
    target: EngineTarget,
) -> Result<Session> {
    let cache_dir = &runtime.engine_cache_dir;
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create engine cache dir {}", cache_dir.display()))?;
    let cache = cache_dir.to_string_lossy().to_string();
    let (min, opt, max) = trt_profile_shapes(kind);

    let int8 = runtime.int8_for(kind == ModelKind::Detector);
    let cache_prefix = trt_cache_prefix(target, int8);

    let trt = TensorRT::default()
        .with_fp16(runtime.fp16)
        // For explicit-QDQ models this lets TensorRT build INT8 kernels; the
        // scales come from the Q/DQ nodes, so no calibration table is needed.
        .with_int8(int8)
        .with_engine_cache(true)
        .with_engine_cache_path(cache.clone())
        .with_engine_cache_prefix(cache_prefix)
        .with_timing_cache(true)
        .with_timing_cache_path(cache)
        .with_profile_min_shapes(min)
        .with_profile_opt_shapes(opt)
        .with_profile_max_shapes(max)
        .build()
        .error_on_failure();
    // CUDA fallback EP for any subgraph TensorRT declines. Failure to register
    // is tolerated here (TensorRT is doing the real work).
    let cuda = tuned_cuda_ep().build();
    Session::builder()
        .map_err(|error| anyhow::anyhow!("failed to create session builder: {error}"))?
        .with_execution_providers([trt, cuda])
        .map_err(|error| anyhow::anyhow!("failed to register TensorRT EP: {error}"))?
        .commit_from_file(path)
        .map_err(|error| anyhow::anyhow!("failed to load model with TensorRT EP: {error}"))
}

/// The CUDA execution provider tuned for this low-latency, dynamic-shape
/// pipeline.
fn tuned_cuda_ep() -> CUDA {
    CUDA::default()
        .with_conv_algorithm_search(ConvAlgorithmSearch::Heuristic)
        .with_tf32(true)
}

fn commit_cuda_session(path: &Path) -> Result<Session> {
    // ort's builder errors embed the (non-Send) `SessionBuilder`, so we stringify
    // them rather than propagating with `?` (which requires Send + Sync + 'static).
    // Here a missing/incompatible CUDA runtime must be a hard error so the caller
    // can fall back to CPU.
    let cuda = tuned_cuda_ep().build().error_on_failure();
    let mut builder = Session::builder()
        .map_err(|error| anyhow::anyhow!("failed to create session builder: {error}"))?
        .with_execution_providers([cuda])
        .map_err(|error| anyhow::anyhow!("failed to register CUDA EP: {error}"))?;
    if std::env::var_os("MITE_ORT_PROFILE").is_some() {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
        builder = builder
            .with_profiling(format!("ort_profile_{stem}"))
            .map_err(|error| anyhow::anyhow!("failed to enable ORT profiling: {error}"))?;
    }
    builder
        .commit_from_file(path)
        .map_err(|error| anyhow::anyhow!("failed to load model with CUDA EP: {error}"))
}

pub(super) fn require_file(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("required model file is missing: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn cache_prefix_encodes_target_and_precision() {
        assert_eq!(
            trt_cache_prefix(EngineTarget::Detector, false),
            format!("mite-detector-max{DET_PROFILE_MAX_SIDE}")
        );
        assert_eq!(
            trt_cache_prefix(EngineTarget::Recognizer, true),
            format!("mite-recognizer-max{REC_MAX_WIDTH}-int8")
        );
        // The fallback recognizer shares the primary's kind and profile but is a
        // different model; a shared prefix would let the primary's cached engine
        // mask the fallback's cold-build probe (and vice versa).
        assert_eq!(
            trt_cache_prefix(EngineTarget::FallbackRecognizer, false),
            format!("mite-fallback-recognizer-max{REC_MAX_WIDTH}")
        );
    }

    #[test]
    fn cache_probe_matches_prefixed_engine_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = trt_cache_prefix(EngineTarget::Detector, false);

        // Empty (or missing) cache dir: cold.
        assert!(!trt_engine_cache_primed(dir.path(), &prefix));
        assert!(!trt_engine_cache_primed(
            &dir.path().join("absent"),
            &prefix
        ));

        // A profile file alone is not a built engine, and another model's engine
        // does not count for this one.
        fs::write(dir.path().join(format!("{prefix}_abc.profile")), b"").unwrap();
        fs::write(dir.path().join("mite-recognizer-max960_abc.engine"), b"").unwrap();
        assert!(!trt_engine_cache_primed(dir.path(), &prefix));

        // The matching engine file flips it to primed.
        fs::write(dir.path().join(format!("{prefix}_abc.engine")), b"").unwrap();
        assert!(trt_engine_cache_primed(dir.path(), &prefix));
    }

    #[test]
    fn expected_build_is_scoped_to_the_tensorrt_backend() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = RuntimeConfig {
            engine_cache_dir: dir.path().to_path_buf(),
            ..RuntimeConfig::default()
        };

        // TensorRT backend with a cold cache: a compile is coming.
        assert!(expects_trt_engine_build(&runtime, EngineTarget::Detector));

        // Same backend with a primed cache: no compile expected.
        let prefix = trt_cache_prefix(EngineTarget::Detector, false);
        fs::write(dir.path().join(format!("{prefix}_abc.engine")), b"").unwrap();
        assert!(!expects_trt_engine_build(&runtime, EngineTarget::Detector));

        // CUDA and CPU backends never TensorRT-compile, cache or no cache.
        for backend in [RuntimeBackend::Cuda, RuntimeBackend::Cpu] {
            let runtime = RuntimeConfig {
                backend,
                engine_cache_dir: PathBuf::from("does-not-exist"),
                ..RuntimeConfig::default()
            };
            assert!(!expects_trt_engine_build(
                &runtime,
                EngineTarget::Recognizer
            ));
        }
    }

    #[test]
    fn primary_recognizer_cache_does_not_mask_the_fallbacks_cold_build() {
        // Regression: with a shared prefix, warming the primary recognizer wrote
        // an engine file that made the fallback's probe report "primed" while
        // TensorRT still had a multi-minute compile ahead for it.
        let dir = tempfile::tempdir().unwrap();
        let runtime = RuntimeConfig {
            engine_cache_dir: dir.path().to_path_buf(),
            ..RuntimeConfig::default()
        };

        let primary = trt_cache_prefix(EngineTarget::Recognizer, false);
        fs::write(dir.path().join(format!("{primary}_abc.engine")), b"").unwrap();

        assert!(!expects_trt_engine_build(
            &runtime,
            EngineTarget::Recognizer
        ));
        assert!(expects_trt_engine_build(
            &runtime,
            EngineTarget::FallbackRecognizer
        ));
    }
}
