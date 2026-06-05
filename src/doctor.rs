use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, RuntimeBackend};
use crate::models::ModelStatus;

const TENSORRT_DLLS: &[&str] = &[
    "nvinfer_10.dll",
    "nvonnxparser_10.dll",
    "nvinfer_plugin_10.dll",
];
const CUDA_DLLS: &[&str] = &[
    "cudart64_12.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cudnn64_9.dll",
    "cudnn_ops64_9.dll",
    "cudnn_cnn64_9.dll",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub os: String,
    pub nvidia: NvidiaStatus,
    pub gpu_runtime: Option<GpuRuntimeStatus>,
    pub runtime_backend: RuntimeBackend,
    pub models: ModelStatus,
    pub warnings: Vec<String>,
}

impl DoctorReport {
    pub fn inspect(config: &AppConfig) -> Self {
        let nvidia = NvidiaStatus::probe();
        let models = ModelStatus::inspect(&config.models);
        let gpu_runtime = GpuRuntimeStatus::inspect(config.runtime.backend);
        let mut warnings = Vec::new();

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

        if let Some(runtime) = &gpu_runtime {
            if !runtime.cache_missing.is_empty() {
                warnings.push(format!(
                    "GPU runtime cache is missing required DLLs: {}. Run scripts\\bootstrap-dev.ps1 -GpuRuntimeOnly.",
                    runtime.cache_missing.join(", ")
                ));
            }
            if !runtime.executable_missing.is_empty() {
                warnings.push(format!(
                    "Current executable directory is missing GPU runtime DLLs: {}. Rebuild after installing the runtime, or rerun scripts\\bootstrap-dev.ps1 -GpuRuntimeOnly to stage existing targets.",
                    runtime.executable_missing.join(", ")
                ));
            }
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
        if let Some(runtime) = &self.gpu_runtime {
            out.push_str(&format!(
                "GPU runtime cache: {} ({} DLLs, {})\n",
                runtime.cache_dir.display(),
                runtime.cache_dll_count,
                missing_label(&runtime.cache_missing)
            ));
            match &runtime.executable_dir {
                Some(path) => out.push_str(&format!(
                    "Executable runtime dir: {} ({} DLLs, {})\n",
                    path.display(),
                    runtime.executable_dll_count,
                    missing_label(&runtime.executable_missing)
                )),
                None => out.push_str("Executable runtime dir: unknown\n"),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuRuntimeStatus {
    pub cache_dir: PathBuf,
    pub executable_dir: Option<PathBuf>,
    pub cache_dll_count: usize,
    pub executable_dll_count: usize,
    pub cache_missing: Vec<String>,
    pub executable_missing: Vec<String>,
}

impl GpuRuntimeStatus {
    pub fn inspect(backend: RuntimeBackend) -> Option<Self> {
        let required = required_gpu_dlls(backend);
        if required.is_empty() {
            return None;
        }

        let cache_dir = gpu_runtime_cache_dir();
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));

        let cache_missing = missing_dlls(&cache_dir, &required);
        let (executable_dll_count, executable_missing) = match &executable_dir {
            Some(path) => (dll_count(path), missing_dlls(path, &required)),
            None => (
                0,
                required
                    .iter()
                    .copied()
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            ),
        };

        Some(Self {
            cache_dll_count: dll_count(&cache_dir),
            cache_dir,
            executable_dir,
            executable_dll_count,
            cache_missing,
            executable_missing,
        })
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

fn missing_label(missing: &[String]) -> String {
    if missing.is_empty() {
        "ready".to_string()
    } else {
        format!("missing {}", missing.join(", "))
    }
}

fn gpu_runtime_cache_dir() -> PathBuf {
    std::env::var_os("MITE_GPU_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".gpu-runtime").join("bin"))
}

fn required_gpu_dlls(backend: RuntimeBackend) -> Vec<&'static str> {
    match backend {
        RuntimeBackend::NvidiaTensorRtThenCuda => TENSORRT_DLLS
            .iter()
            .chain(CUDA_DLLS.iter())
            .copied()
            .collect(),
        RuntimeBackend::Cuda => CUDA_DLLS.to_vec(),
        RuntimeBackend::Mock | RuntimeBackend::DirectMl | RuntimeBackend::OpenVino => Vec::new(),
    }
}

fn missing_dlls(dir: &Path, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .copied()
        .filter(|name| !dir.join(name).is_file())
        .map(str::to_string)
        .collect()
}

fn dll_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_runtime_requirements_follow_backend() {
        assert!(required_gpu_dlls(RuntimeBackend::Mock).is_empty());
        assert_eq!(
            required_gpu_dlls(RuntimeBackend::Cuda),
            vec![
                "cudart64_12.dll",
                "cublas64_12.dll",
                "cublasLt64_12.dll",
                "cudnn64_9.dll",
                "cudnn_ops64_9.dll",
                "cudnn_cnn64_9.dll",
            ]
        );

        let trt = required_gpu_dlls(RuntimeBackend::NvidiaTensorRtThenCuda);
        assert!(trt.contains(&"nvinfer_10.dll"));
        assert!(trt.contains(&"cudnn64_9.dll"));
    }

    #[test]
    fn missing_dlls_reports_only_absent_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cudart64_12.dll"), b"").unwrap();

        assert_eq!(
            missing_dlls(dir.path(), &["cudart64_12.dll", "cudnn64_9.dll"]),
            vec!["cudnn64_9.dll"]
        );
    }
}
