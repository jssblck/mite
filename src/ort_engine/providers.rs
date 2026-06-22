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
    DET_PROFILE_OPT_WIDTH, REC_BATCH_MAX, REC_CHANNELS, REC_INPUT_HEIGHT, REC_MAX_WIDTH,
    REC_MIN_WIDTH, REC_OPT_WIDTH,
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

/// Build a session for `path`, preferring the fastest execution provider the
/// runtime asks for and degrading gracefully: TensorRT -> CUDA -> CPU. Each GPU
/// provider registers with `error_on_failure`, so a missing/incompatible runtime
/// surfaces as a logged warning and we fall back rather than silently running
/// slow with no explanation.
pub(super) fn commit_session(
    path: &Path,
    runtime: &RuntimeConfig,
    kind: ModelKind,
) -> Result<Session> {
    ensure_ort();

    let label = kind.label();
    let try_trt = matches!(runtime.backend, RuntimeBackend::NvidiaTensorRtThenCuda);
    let try_cuda = matches!(
        runtime.backend,
        RuntimeBackend::NvidiaTensorRtThenCuda | RuntimeBackend::Cuda
    );

    if try_trt {
        match commit_trt_session(path, runtime, kind) {
            Ok(session) => {
                tracing::info!(
                    "{label}: TensorRT execution provider active ({})",
                    path.display()
                );
                return Ok(session);
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
                return Ok(session);
            }
            Err(error) => {
                tracing::warn!(
                    "{label}: CUDA execution provider unavailable ({error}); falling back to CPU. \
                     Install the NVIDIA CUDA/cuDNN runtime and ensure it is on PATH, then run `mite doctor` to confirm the tier."
                );
            }
        }
    }

    Session::builder()?
        .commit_from_file(path)
        .with_context(|| format!("failed to load {label} model {}", path.display()))
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
fn commit_trt_session(path: &Path, runtime: &RuntimeConfig, kind: ModelKind) -> Result<Session> {
    let cache_dir = &runtime.engine_cache_dir;
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create engine cache dir {}", cache_dir.display()))?;
    let cache = cache_dir.to_string_lossy().to_string();
    let (min, opt, max) = trt_profile_shapes(kind);

    let int8 = runtime.int8_for(kind == ModelKind::Detector);
    let mut cache_prefix = match kind {
        ModelKind::Detector => format!("mite-detector-max{DET_PROFILE_MAX_SIDE}"),
        ModelKind::Recognizer => format!("mite-recognizer-max{REC_MAX_WIDTH}"),
    };
    if int8 {
        cache_prefix.push_str("-int8");
    }

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
