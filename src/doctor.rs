use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, RuntimeBackend};
use crate::models::ModelStatus;

/// TensorRT-tier DLLs beyond the CUDA set: the inference runtime, the ONNX
/// parser, and the plugin library. The `ort` build links `nvinfer_10.dll`
/// (TensorRT major 10), so detection and guidance steer to the 10.x runtime.
const TENSORRT_DLLS: &[&str] = &[
    "nvinfer_10.dll",
    "nvonnxparser_10.dll",
    "nvinfer_plugin_10.dll",
];
/// The on-device TensorRT engine builder component. It is only needed to build
/// an engine on this machine; a cached engine runs without it, so it is reported
/// on its own rather than gating the tier. It ships under more than one name:
/// NVIDIA's standalone TensorRT package historically used a single
/// `nvinfer_builder_resource.dll`, while the pip wheels split it per compute
/// capability (for example `nvinfer_builder_resource_sm89_10.dll`). Both start
/// with this prefix, so it is matched by prefix rather than an exact file name.
const TENSORRT_BUILDER_PREFIX: &str = "nvinfer_builder_resource";
/// CUDA-tier DLLs: the CUDA 12 runtime, cuBLAS, and cuDNN 9. These majors match
/// what the `ort` build links, so guidance pins compatible major versions.
const CUDA_DLLS: &[&str] = &[
    "cudart64_12.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cudnn64_9.dll",
    "cudnn_ops64_9.dll",
    "cudnn_cnn64_9.dll",
];
/// NVRTC ships under a versioned file name (for example `nvrtc64_120_0.dll`), so
/// it is matched by prefix rather than an exact file name.
const NVRTC_PREFIX: &str = "nvrtc64_";

/// ONNX Runtime's execution-provider bridge DLLs. Unlike the NVIDIA runtime,
/// these ship WITH Mite: they are part of ONNX Runtime (MIT-licensed Microsoft
/// software, not NVIDIA binaries) and the `ort` build emits them next to
/// `mite.exe`. They must sit beside the engine for any GPU execution provider to
/// load. `onnxruntime_providers_shared.dll` is the bridge every GPU EP needs;
/// without it ONNX Runtime cannot register CUDA or TensorRT and silently falls
/// back to the CPU even when the full NVIDIA runtime is installed. A missing
/// entry here is therefore an engine-packaging problem, not a user install gap.
const ORT_SHARED_DLL: &str = "onnxruntime_providers_shared.dll";
const ORT_CUDA_DLL: &str = "onnxruntime_providers_cuda.dll";
const ORT_TENSORRT_DLL: &str = "onnxruntime_providers_tensorrt.dll";

/// The execution tier the detected NVIDIA runtime can support, slowest-safe to
/// fastest. This is the app-facing summary of `GpuRuntimeStatus`; it maps to a
/// concrete `RuntimeBackend` for launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTier {
    /// No usable NVIDIA runtime was found. The CPU provider always works.
    Cpu,
    /// The CUDA runtime set is present but TensorRT is not. Roughly 2x slower
    /// than TensorRT, much faster than CPU.
    Cuda,
    /// The full TensorRT set plus its CUDA fallback is present: the fast path.
    TensorRt,
}

impl RuntimeTier {
    /// The launch backend for this tier. CPU maps to the explicit CPU backend so
    /// the app does not log TensorRT/CUDA registration failures on machines with
    /// no NVIDIA runtime; CUDA maps to the CUDA-only backend so the UI does not
    /// imply TensorRT is active.
    pub fn to_backend(self) -> RuntimeBackend {
        match self {
            RuntimeTier::Cpu => RuntimeBackend::Cpu,
            RuntimeTier::Cuda => RuntimeBackend::Cuda,
            RuntimeTier::TensorRt => RuntimeBackend::NvidiaTensorRtThenCuda,
        }
    }
}

/// Pick the best tier from which required DLL sets are fully present. TensorRT
/// needs both its own DLLs and the CUDA fallback set, since the TensorRT EP
/// co-registers CUDA for any subgraph it declines.
fn decide_tier(tensorrt_present: bool, cuda_present: bool) -> RuntimeTier {
    if tensorrt_present && cuda_present {
        RuntimeTier::TensorRt
    } else if cuda_present {
        RuntimeTier::Cuda
    } else {
        RuntimeTier::Cpu
    }
}

/// Reduce the NVIDIA-supported tier to the tier the engine can actually reach,
/// given which ONNX Runtime provider DLLs are present next to it. The NVIDIA
/// runtime alone is not enough: ONNX Runtime needs its own provider bridge to
/// register any GPU execution provider, and that bridge ships with the engine,
/// not with NVIDIA. A TensorRT-capable machine missing the TensorRT provider DLL
/// can still fall back to CUDA; one missing the shared bridge runs on the CPU.
fn effective_tier(nvidia_tier: RuntimeTier, ort: &OrtProviderStatus) -> RuntimeTier {
    match nvidia_tier {
        RuntimeTier::TensorRt if ort.tensorrt_ready() => RuntimeTier::TensorRt,
        RuntimeTier::TensorRt | RuntimeTier::Cuda if ort.cuda_ready() => RuntimeTier::Cuda,
        _ => RuntimeTier::Cpu,
    }
}

/// The ONNX Runtime provider gap warning, if any: the NVIDIA runtime can support
/// a faster tier than the engine can currently reach because Mite's ONNX Runtime
/// provider DLLs are missing next to it. This is the actionable misconfiguration
/// behind a "TensorRT detected but running on CPU" report.
fn ort_provider_gap_warning(runtime: &GpuRuntimeStatus) -> Option<String> {
    if runtime.effective_tier == runtime.tier {
        return None;
    }
    Some(format!(
        "The installed NVIDIA runtime supports the {} tier, but Mite's ONNX Runtime \
         provider libraries are missing next to the engine ({}), so the overlay will run \
         at the {} tier. Update or reinstall the Mite engine to restore GPU acceleration.",
        tier_name(runtime.tier),
        runtime
            .ort_providers
            .missing_for_tier(runtime.tier)
            .join(", "),
        tier_name(runtime.effective_tier),
    ))
}

/// A friendly name for a tier, for human-facing warnings and text output.
fn tier_name(tier: RuntimeTier) -> &'static str {
    match tier {
        RuntimeTier::Cpu => "CPU",
        RuntimeTier::Cuda => "CUDA",
        RuntimeTier::TensorRt => "TensorRT",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub os: String,
    pub nvidia: NvidiaStatus,
    pub gpu_runtime: GpuRuntimeStatus,
    pub runtime_backend: RuntimeBackend,
    pub models: ModelStatus,
    pub warnings: Vec<String>,
}

impl DoctorReport {
    pub fn inspect(config: &AppConfig) -> Self {
        let nvidia = NvidiaStatus::probe();
        let models = ModelStatus::inspect(&config.models);
        let gpu_runtime = GpuRuntimeStatus::detect();
        let mut warnings = Vec::new();

        // Surface the gap between what hardware is present and what runtime was
        // found, in plain language. The desktop app turns this into the guided
        // install flow; the CLI just reports it.
        if nvidia.available {
            match gpu_runtime.tier {
                RuntimeTier::Cpu => warnings.push(
                    "An NVIDIA GPU was detected but no NVIDIA GPU runtime (TensorRT or CUDA) \
                     was found, so Mite will run on the CPU. Install NVIDIA's runtime to enable GPU \
                     acceleration and ensure it is on PATH (the desktop app guides this; for local \
                     development see docs/local-windows.md)."
                        .to_string(),
                ),
                RuntimeTier::Cuda => warnings.push(
                    "The CUDA runtime was found but TensorRT was not, so Mite will use the CUDA \
                     backend (roughly 2x slower than TensorRT). Install the TensorRT 10.x runtime \
                     to enable the fastest path."
                        .to_string(),
                ),
                RuntimeTier::TensorRt => {
                    if !gpu_runtime.builder_present {
                        warnings.push(
                            "The TensorRT runtime is present but nvinfer_builder_resource.dll was \
                             not found. A previously built engine cache will still run; building a \
                             new engine on this machine needs the builder component from NVIDIA's \
                             TensorRT package."
                                .to_string(),
                        );
                    }
                }
            }
        }

        // The NVIDIA runtime can support a GPU tier, but ONNX Runtime needs its
        // own provider DLLs next to the engine to use it. When those are missing
        // the overlay silently runs on the CPU even though `doctor` would
        // otherwise report a green TensorRT/CUDA tier; surface that gap plainly.
        warnings.extend(ort_provider_gap_warning(&gpu_runtime));

        if matches!(
            config.runtime.backend,
            RuntimeBackend::NvidiaTensorRtThenCuda | RuntimeBackend::Cuda
        ) && !nvidia.available
        {
            warnings.push("NVIDIA backend selected, but nvidia-smi was not available.".to_string());
        }

        if !models.ready_for_real_inference() {
            warnings.push(format!(
                "Model files are missing: {}",
                models
                    .missing_paths()
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Self {
            os: std::env::consts::OS.to_string(),
            nvidia,
            gpu_runtime,
            runtime_backend: config.runtime.backend,
            models,
            warnings,
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("Mite doctor\n");
        out.push_str(&format!("OS: {}\n", self.os));
        out.push_str(&format!(
            "NVIDIA: {}\n",
            if self.nvidia.available {
                "available"
            } else {
                "not available"
            }
        ));
        if let Some(name) = &self.nvidia.gpu_name {
            out.push_str(&format!("GPU: {name}\n"));
        }
        if let Some(driver) = &self.nvidia.driver_version {
            out.push_str(&format!("Driver: {driver}\n"));
        }
        out.push_str(&format!("Runtime backend: {:?}\n", self.runtime_backend));

        let runtime = &self.gpu_runtime;
        out.push_str(&format!(
            "NVIDIA-supported tier: {}\n",
            tier_name(runtime.tier)
        ));
        out.push_str(&format!(
            "Effective tier (engine runs at): {}\n",
            tier_name(runtime.effective_tier)
        ));
        out.push_str(&format!(
            "TensorRT runtime: {}\n",
            tier_label(&runtime.tensorrt)
        ));
        out.push_str(&format!("CUDA runtime: {}\n", tier_label(&runtime.cuda)));
        out.push_str(&format!(
            "ONNX Runtime providers: shared {}, cuda {}, tensorrt {}\n",
            exists_label(runtime.ort_providers.shared.present),
            exists_label(runtime.ort_providers.cuda.present),
            exists_label(runtime.ort_providers.tensorrt.present),
        ));
        out.push_str(&format!(
            "TensorRT engine builder: {}\n",
            exists_label(runtime.builder_present)
        ));
        out.push_str(&format!("NVRTC: {}\n", exists_label(runtime.nvrtc_present)));
        if runtime.dll_dirs.is_empty() {
            out.push_str("Runtime DLL directories: none found\n");
        } else {
            out.push_str("Runtime DLL directories:\n");
            for dir in &runtime.dll_dirs {
                out.push_str(&format!("- {}\n", dir.display()));
            }
        }

        out.push_str(&format!(
            "Detector model: {} ({})\n",
            self.models.detector_path.display(),
            exists_label(self.models.detector_exists)
        ));
        out.push_str(&format!(
            "Recognizer model: {} ({})\n",
            self.models.recognizer_path.display(),
            exists_label(self.models.recognizer_exists)
        ));
        if let Some(path) = &self.models.charset_path {
            out.push_str(&format!(
                "Charset: {} ({})\n",
                path.display(),
                exists_label(self.models.charset_exists.unwrap_or(false))
            ));
        }
        if self.warnings.is_empty() {
            out.push_str("Warnings: none\n");
        } else {
            out.push_str("Warnings:\n");
            for warning in &self.warnings {
                out.push_str(&format!("- {warning}\n"));
            }
        }
        out
    }
}

/// Presence of one required runtime DLL, and which directory satisfied it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DllPresence {
    pub name: String,
    pub present: bool,
    pub found_in: Option<PathBuf>,
}

/// Presence of a whole tier's required DLL set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierStatus {
    /// True when every component of the tier was found.
    pub present: bool,
    pub components: Vec<DllPresence>,
}

impl TierStatus {
    /// Names of the components that are still missing.
    pub fn missing(&self) -> Vec<&str> {
        self.components
            .iter()
            .filter(|component| !component.present)
            .map(|component| component.name.as_str())
            .collect()
    }
}

/// Presence of ONNX Runtime's execution-provider bridge DLLs next to the engine.
///
/// These are shipped by Mite itself (ONNX Runtime, MIT), so a missing entry here
/// is an engine-packaging problem, not something the user installs from NVIDIA.
/// When they are absent the engine cannot reach any GPU tier regardless of the
/// detected NVIDIA runtime, which is why `effective_tier` gates on them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrtProviderStatus {
    /// The shared bridge every GPU EP loads (`onnxruntime_providers_shared.dll`).
    pub shared: DllPresence,
    /// The CUDA execution-provider library.
    pub cuda: DllPresence,
    /// The TensorRT execution-provider library.
    pub tensorrt: DllPresence,
}

impl OrtProviderStatus {
    /// CUDA registers only when both the shared bridge and the CUDA EP are present.
    fn cuda_ready(&self) -> bool {
        self.shared.present && self.cuda.present
    }

    /// TensorRT needs the shared bridge and its own EP. It does NOT need the CUDA
    /// provider: `commit_trt_session` registers TensorRT as the hard requirement
    /// and CUDA only as a tolerated fallback, so the engine reaches TensorRT with
    /// just these two present (any subgraph TensorRT declines runs on the CPU EP).
    fn tensorrt_ready(&self) -> bool {
        self.shared.present && self.tensorrt.present
    }

    /// The provider DLLs required to reach `tier` that are currently missing.
    /// Scoped to the tier so a CUDA-only machine is not told it is missing the
    /// TensorRT provider, which it does not need.
    fn missing_for_tier(&self, tier: RuntimeTier) -> Vec<&str> {
        let required: &[&DllPresence] = match tier {
            RuntimeTier::Cpu => &[],
            RuntimeTier::Cuda => &[&self.shared, &self.cuda],
            RuntimeTier::TensorRt => &[&self.shared, &self.tensorrt],
        };
        required
            .iter()
            .filter(|component| !component.present)
            .map(|component| component.name.as_str())
            .collect()
    }
}

/// The result of searching the system for the NVIDIA runtime DLLs Mite needs.
///
/// Mite never downloads, hosts, or installs these: it only detects what the user
/// has installed from NVIDIA, decides which tier that supports, and records the
/// directories the launcher must put on the CLI's `PATH`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuRuntimeStatus {
    /// The best tier the detected NVIDIA DLLs support. This reflects only the
    /// user's NVIDIA install, which is what the guided runtime setup cares about.
    pub tier: RuntimeTier,
    /// The tier the engine can actually reach right now: `tier` gated by whether
    /// Mite's ONNX Runtime provider DLLs (`ort_providers`) are present next to
    /// the engine. This is what the overlay really runs at, so the desktop app's
    /// status surface should read this rather than `tier`.
    pub effective_tier: RuntimeTier,
    /// The TensorRT-tier DLLs (beyond the shared CUDA set).
    pub tensorrt: TierStatus,
    /// The CUDA-tier DLLs.
    pub cuda: TierStatus,
    /// ONNX Runtime's provider bridge DLLs next to the engine. Shipped by Mite,
    /// required for any GPU EP to load. `tier` ignores these; `effective_tier`
    /// does not.
    pub ort_providers: OrtProviderStatus,
    /// Whether the on-device TensorRT engine builder component is present.
    pub builder_present: bool,
    /// Whether a versioned NVRTC DLL is present.
    pub nvrtc_present: bool,
    /// Directories that actually held a required DLL: what the launcher prepends
    /// to the spawned CLI's `PATH` so the loader can resolve the runtime.
    pub dll_dirs: Vec<PathBuf>,
    /// Every directory that was searched, for diagnostics.
    pub searched_dirs: Vec<PathBuf>,
}

impl GpuRuntimeStatus {
    /// Detect across the real system search path (PATH, NVIDIA toolkit installs,
    /// pip wheel layouts, the mite-managed cache, and any user-specified dirs).
    pub fn detect() -> Self {
        Self::detect_in(&runtime_search_dirs())
    }

    /// Detect across an explicit directory list. Pure over `dirs` (it only stats
    /// files), so the search-and-decide logic is unit-testable.
    pub fn detect_in(dirs: &[PathBuf]) -> Self {
        let resolve = |name: &str| dirs.iter().find(|dir| dir.join(name).is_file()).cloned();

        let tensorrt = tier_status(TENSORRT_DLLS, &resolve);
        let cuda = tier_status(CUDA_DLLS, &resolve);
        // ONNX Runtime's own provider bridge DLLs. The `ort` build emits these
        // next to the engine, and the exe directory is part of the search set, so
        // they resolve from there for the installed app exactly as the OS loader
        // would find them.
        let ort_providers = OrtProviderStatus {
            shared: dll_presence(ORT_SHARED_DLL, &resolve),
            cuda: dll_presence(ORT_CUDA_DLL, &resolve),
            tensorrt: dll_presence(ORT_TENSORRT_DLL, &resolve),
        };
        // The builder resource has a versioned, per-SM file name in the pip
        // wheels, so it is matched by prefix across the TensorRT directories.
        let builder_dir = dirs
            .iter()
            .find(|dir| dir_has_prefixed_dll(dir, TENSORRT_BUILDER_PREFIX))
            .cloned();

        // Collect the directories that supplied a required DLL.
        let mut found_dirs: BTreeSet<PathBuf> = BTreeSet::new();
        for component in tensorrt.components.iter().chain(cuda.components.iter()) {
            if let Some(dir) = &component.found_in {
                found_dirs.insert(dir.clone());
            }
        }
        if let Some(dir) = &builder_dir {
            found_dirs.insert(dir.clone());
        }

        // NVRTC has a versioned name, so scan only the directories already known
        // to hold the runtime (it ships alongside the CUDA runtime) rather than
        // listing every search directory.
        let nvrtc_dir = found_dirs
            .iter()
            .find(|dir| dir_has_prefixed_dll(dir, NVRTC_PREFIX))
            .cloned();
        if let Some(dir) = &nvrtc_dir {
            found_dirs.insert(dir.clone());
        }

        let tier = decide_tier(tensorrt.present, cuda.present);
        Self {
            tier,
            effective_tier: effective_tier(tier, &ort_providers),
            tensorrt,
            cuda,
            ort_providers,
            builder_present: builder_dir.is_some(),
            nvrtc_present: nvrtc_dir.is_some(),
            dll_dirs: found_dirs.into_iter().collect(),
            searched_dirs: dirs.to_vec(),
        }
    }
}

/// Resolve a single DLL by name across the search dirs into a `DllPresence`.
fn dll_presence(name: &str, resolve: &impl Fn(&str) -> Option<PathBuf>) -> DllPresence {
    let found_in = resolve(name);
    DllPresence {
        name: name.to_string(),
        present: found_in.is_some(),
        found_in,
    }
}

fn tier_status(required: &[&str], resolve: &impl Fn(&str) -> Option<PathBuf>) -> TierStatus {
    let components: Vec<DllPresence> = required
        .iter()
        .map(|name| dll_presence(name, resolve))
        .collect();
    let present = components.iter().all(|component| component.present);
    TierStatus {
        present,
        components,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvidiaStatus {
    pub available: bool,
    pub gpu_name: Option<String>,
    pub driver_version: Option<String>,
    pub raw_error: Option<String>,
}

impl NvidiaStatus {
    pub fn probe() -> Self {
        let output = Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,driver_version",
                "--format=csv,noheader,nounits",
            ])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let first = stdout.lines().next().unwrap_or_default();
                let mut parts = first.split(',').map(str::trim);
                Self {
                    available: true,
                    gpu_name: parts
                        .next()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    driver_version: parts
                        .next()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    raw_error: None,
                }
            }
            Ok(output) => Self {
                available: false,
                gpu_name: None,
                driver_version: None,
                raw_error: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            },
            Err(error) => Self {
                available: false,
                gpu_name: None,
                driver_version: None,
                raw_error: Some(error.to_string()),
            },
        }
    }
}

fn exists_label(exists: bool) -> &'static str {
    if exists { "found" } else { "missing" }
}

fn tier_label(tier: &TierStatus) -> String {
    if tier.present {
        "ready".to_string()
    } else {
        format!("missing {}", tier.missing().join(", "))
    }
}

/// An optional drop-in GPU runtime directory: `MITE_GPU_RUNTIME_DIR` if set
/// (the desktop app points this at a per-user folder), otherwise a local
/// `.gpu-runtime\bin` a contributor can drop the NVIDIA DLLs into. Mite never
/// populates this itself.
fn gpu_runtime_cache_dir() -> PathBuf {
    std::env::var_os("MITE_GPU_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".gpu-runtime").join("bin"))
}

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

/// Build the ordered list of directories to search for the NVIDIA runtime. This
/// deliberately spans both NVIDIA's official install routes (the CUDA Toolkit,
/// cuDNN, and TensorRT on the system, reachable through `CUDA_PATH`, Program
/// Files, and `PATH`) and the pip wheel route (`nvidia\*\bin` and
/// `tensorrt_libs` under a target or venv). A user-specified folder is honored
/// via `MITE_GPU_RUNTIME_EXTRA_DIRS`.
fn runtime_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // The mite-managed cache and the executable's own directory.
    push_dir(&mut dirs, gpu_runtime_cache_dir());
    if let Some(exe_dir) = current_exe_dir() {
        push_dir(&mut dirs, exe_dir);
    }

    // A dedicated folder the user can pip-install the NVIDIA wheels into. It is
    // resolved against the working directory: the desktop app launches the CLI
    // from the mite home, so `nvidia-runtime` lands inside the home.
    push_wheel_layout(&mut dirs, PathBuf::from("nvidia-runtime"));
    // A common local venv name for contributors who pip-install the NVIDIA wheels.
    push_wheel_layout(&mut dirs, PathBuf::from(".venv-models"));

    // Any user-specified extra folders (a less common install location).
    if let Some(extra) = std::env::var_os("MITE_GPU_RUNTIME_EXTRA_DIRS") {
        for path in std::env::split_paths(&extra) {
            push_wheel_layout(&mut dirs, path);
        }
    }

    // CUDA Toolkit installs expose their bin directory through CUDA_PATH and the
    // versioned CUDA_PATH_V12_* variables.
    for (key, value) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key == "CUDA_PATH" || key.starts_with("CUDA_PATH_V12") {
            push_dir(&mut dirs, PathBuf::from(value).join("bin"));
        }
    }

    // The default CUDA Toolkit location and the usual TensorRT/cuDNN unzip roots
    // under Program Files.
    for program_files_key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(program_files_key) {
            let program_files = PathBuf::from(program_files);
            push_cuda_toolkit_dirs(&mut dirs, &program_files);
            push_nvidia_lib_dirs(&mut dirs, &program_files.join("NVIDIA"));
        }
    }

    // Everything already on PATH (covers system installs and any directory the
    // user added by hand).
    if let Some(path) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path) {
            push_dir(&mut dirs, path);
        }
    }

    dirs
}

fn push_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// Add `base` plus the nested layouts pip uses when the NVIDIA runtime wheels
/// are installed there, both with `pip install --target <base>` and inside a
/// venv rooted at `<base>`.
fn push_wheel_layout(dirs: &mut Vec<PathBuf>, base: PathBuf) {
    push_dir(dirs, base.clone());
    push_dir(dirs, base.join("tensorrt_libs"));
    push_dir(
        dirs,
        base.join("Lib").join("site-packages").join("tensorrt_libs"),
    );
    for nvidia_root in [
        base.join("nvidia"),
        base.join("Lib").join("site-packages").join("nvidia"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&nvidia_root) {
            for entry in entries.filter_map(Result::ok) {
                push_dir(dirs, entry.path().join("bin"));
            }
        }
    }
}

fn push_cuda_toolkit_dirs(dirs: &mut Vec<PathBuf>, program_files: &Path) {
    let cuda_root = program_files
        .join("NVIDIA GPU Computing Toolkit")
        .join("CUDA");
    if let Ok(entries) = std::fs::read_dir(&cuda_root) {
        for entry in entries.filter_map(Result::ok) {
            push_dir(dirs, entry.path().join("bin"));
        }
    }
}

fn push_nvidia_lib_dirs(dirs: &mut Vec<PathBuf>, nvidia_root: &Path) {
    if let Ok(entries) = std::fs::read_dir(nvidia_root) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            // TensorRT zips extract DLLs under lib\; cuDNN zips under bin\.
            push_dir(dirs, path.join("bin"));
            push_dir(dirs, path.join("lib"));
        }
    }
}

/// True when `dir` holds a `.dll` whose name starts with `prefix` (used for the
/// versioned NVRTC file name).
fn dir_has_prefixed_dll(dir: &Path, prefix: &str) -> bool {
    let prefix = prefix.to_ascii_lowercase();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        name.starts_with(&prefix) && name.ends_with(".dll")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"").unwrap();
    }

    #[test]
    fn tier_decision_follows_present_dll_sets() {
        assert_eq!(decide_tier(true, true), RuntimeTier::TensorRt);
        assert_eq!(decide_tier(false, true), RuntimeTier::Cuda);
        assert_eq!(decide_tier(false, false), RuntimeTier::Cpu);
        // TensorRT DLLs without the CUDA fallback set is not a usable TensorRT
        // tier; the chain needs CUDA co-registered.
        assert_eq!(decide_tier(true, false), RuntimeTier::Cpu);
    }

    #[test]
    fn tier_maps_to_launch_backend() {
        assert_eq!(RuntimeTier::Cpu.to_backend(), RuntimeBackend::Cpu);
        assert_eq!(RuntimeTier::Cuda.to_backend(), RuntimeBackend::Cuda);
        assert_eq!(
            RuntimeTier::TensorRt.to_backend(),
            RuntimeBackend::NvidiaTensorRtThenCuda
        );
    }

    #[test]
    fn detect_in_reports_cpu_when_nothing_present() {
        let dir = tempfile::tempdir().unwrap();
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::Cpu);
        assert!(!status.cuda.present);
        assert!(!status.tensorrt.present);
        assert!(status.dll_dirs.is_empty());
        assert_eq!(
            status.cuda.missing(),
            vec![
                "cudart64_12.dll",
                "cublas64_12.dll",
                "cublasLt64_12.dll",
                "cudnn64_9.dll",
                "cudnn_ops64_9.dll",
                "cudnn_cnn64_9.dll",
            ]
        );
    }

    #[test]
    fn detect_in_reports_cuda_tier_when_only_cuda_present() {
        let dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(dir.path(), name);
        }
        touch(dir.path(), "nvrtc64_120_0.dll");

        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::Cuda);
        assert!(status.cuda.present);
        assert!(!status.tensorrt.present);
        assert!(status.nvrtc_present);
        assert_eq!(status.dll_dirs, vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn detect_in_reports_tensorrt_tier_and_records_dirs() {
        // CUDA DLLs in one directory, TensorRT DLLs in another: both directories
        // must be recorded so the launcher can add them to PATH.
        let cuda_dir = tempfile::tempdir().unwrap();
        let trt_dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(cuda_dir.path(), name);
        }
        for name in TENSORRT_DLLS {
            touch(trt_dir.path(), name);
        }
        // The pip wheels ship the builder resource per compute capability; the
        // prefix match must accept that name, not just the legacy single file.
        touch(trt_dir.path(), "nvinfer_builder_resource_sm89_10.dll");

        let dirs = vec![cuda_dir.path().to_path_buf(), trt_dir.path().to_path_buf()];
        let status = GpuRuntimeStatus::detect_in(&dirs);
        assert_eq!(status.tier, RuntimeTier::TensorRt);
        assert!(status.tensorrt.present);
        assert!(status.cuda.present);
        assert!(status.builder_present);
        assert!(status.dll_dirs.contains(&cuda_dir.path().to_path_buf()));
        assert!(status.dll_dirs.contains(&trt_dir.path().to_path_buf()));
    }

    #[test]
    fn detect_in_accepts_legacy_builder_resource_name() {
        // NVIDIA's standalone TensorRT package uses the single-file name; it must
        // still satisfy the builder check.
        let cuda_dir = tempfile::tempdir().unwrap();
        let trt_dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(cuda_dir.path(), name);
        }
        for name in TENSORRT_DLLS {
            touch(trt_dir.path(), name);
        }
        touch(trt_dir.path(), "nvinfer_builder_resource.dll");

        let dirs = vec![cuda_dir.path().to_path_buf(), trt_dir.path().to_path_buf()];
        let status = GpuRuntimeStatus::detect_in(&dirs);
        assert!(status.builder_present);
    }

    #[test]
    fn effective_tier_gates_on_ort_providers() {
        // A full NVIDIA runtime but no ONNX Runtime provider DLLs: the user's
        // install supports TensorRT, but the engine can only reach the CPU. This
        // is exactly the "TensorRT detected, running on CPU" disconnect.
        let dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(dir.path(), name);
        }
        for name in TENSORRT_DLLS {
            touch(dir.path(), name);
        }
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::TensorRt);
        assert_eq!(status.effective_tier, RuntimeTier::Cpu);
        assert!(!status.ort_providers.shared.present);

        // Drop in the ONNX Runtime provider DLLs and the engine reaches TensorRT.
        touch(dir.path(), ORT_SHARED_DLL);
        touch(dir.path(), ORT_CUDA_DLL);
        touch(dir.path(), ORT_TENSORRT_DLL);
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::TensorRt);
        assert_eq!(status.effective_tier, RuntimeTier::TensorRt);
        assert!(status.ort_providers.tensorrt.present);
    }

    #[test]
    fn effective_tier_reaches_tensorrt_without_the_cuda_provider() {
        // The engine registers TensorRT as a hard requirement and CUDA only as a
        // tolerated fallback, so shared + tensorrt providers are enough to reach
        // TensorRT even with the CUDA provider DLL absent. Doctor must agree.
        let dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(dir.path(), name);
        }
        for name in TENSORRT_DLLS {
            touch(dir.path(), name);
        }
        touch(dir.path(), ORT_SHARED_DLL);
        touch(dir.path(), ORT_TENSORRT_DLL);
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::TensorRt);
        assert_eq!(status.effective_tier, RuntimeTier::TensorRt);
        // No gap, so no warning, even though the CUDA provider is absent.
        assert!(ort_provider_gap_warning(&status).is_none());
    }

    #[test]
    fn effective_tier_degrades_tensorrt_to_cuda_without_trt_provider() {
        // NVIDIA supports TensorRT and the shared+CUDA provider bridge is present,
        // but the TensorRT provider DLL is not: the engine can still run on CUDA.
        let dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(dir.path(), name);
        }
        for name in TENSORRT_DLLS {
            touch(dir.path(), name);
        }
        touch(dir.path(), ORT_SHARED_DLL);
        touch(dir.path(), ORT_CUDA_DLL);
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::TensorRt);
        assert_eq!(status.effective_tier, RuntimeTier::Cuda);
    }

    #[test]
    fn ort_provider_gap_warning_fires_only_on_a_real_gap() {
        // CUDA supported by NVIDIA, but no ONNX Runtime providers: the warning
        // names the shortfall and the fix.
        let dir = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(dir.path(), name);
        }
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        let warning = ort_provider_gap_warning(&status).expect("a gap warning");
        assert!(warning.contains("ONNX Runtime"));
        assert!(warning.contains("CUDA"));
        assert!(warning.contains(ORT_SHARED_DLL));
        // A CUDA-only machine does not need the TensorRT provider, so the warning
        // must not list it as missing.
        assert!(!warning.contains(ORT_TENSORRT_DLL));

        // Add the providers CUDA needs: no gap, no warning.
        touch(dir.path(), ORT_SHARED_DLL);
        touch(dir.path(), ORT_CUDA_DLL);
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert!(ort_provider_gap_warning(&status).is_none());
    }

    #[test]
    fn no_gap_warning_when_no_nvidia_runtime() {
        // Nothing installed at all: the effective tier equals the (CPU) tier, so
        // there is no provider gap to warn about. The "no NVIDIA runtime" guidance
        // is a separate warning produced by `inspect`.
        let dir = tempfile::tempdir().unwrap();
        let status = GpuRuntimeStatus::detect_in(&[dir.path().to_path_buf()]);
        assert_eq!(status.tier, RuntimeTier::Cpu);
        assert_eq!(status.effective_tier, RuntimeTier::Cpu);
        assert!(ort_provider_gap_warning(&status).is_none());
    }

    #[test]
    fn detect_in_finds_dlls_across_multiple_dirs() {
        // A required DLL present in a later directory still counts.
        let empty = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        for name in CUDA_DLLS {
            touch(real.path(), name);
        }
        let dirs = vec![empty.path().to_path_buf(), real.path().to_path_buf()];
        let status = GpuRuntimeStatus::detect_in(&dirs);
        assert!(status.cuda.present);
        assert_eq!(
            status.cuda.components[0].found_in.as_deref(),
            Some(real.path())
        );
    }

    #[test]
    fn doctor_report_serializes_to_json() {
        // The desktop app parses `mite doctor --json`, so the report must
        // serialize to a stable object with the keys it reads.
        let report = DoctorReport::inspect(&AppConfig::default());
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert!(value.get("os").is_some());
        assert!(value.get("nvidia").is_some());
        assert!(value.get("models").is_some());
        assert!(value.get("runtime_backend").is_some());
        assert!(value.get("warnings").unwrap().is_array());
        let gpu = value.get("gpu_runtime").unwrap();
        assert!(gpu.get("tier").is_some());
        assert!(gpu.get("effective_tier").is_some());
        assert!(
            gpu.get("cuda")
                .unwrap()
                .get("components")
                .unwrap()
                .is_array()
        );
        assert!(gpu.get("dll_dirs").unwrap().is_array());
        let ort = gpu.get("ort_providers").unwrap();
        assert!(ort.get("shared").unwrap().get("present").is_some());
        assert!(ort.get("cuda").unwrap().get("present").is_some());
        assert!(ort.get("tensorrt").unwrap().get("present").is_some());
    }
}
