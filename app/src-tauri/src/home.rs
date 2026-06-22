//! The app-owned "mite home" directory and its layout.
//!
//! Everything the mite CLI needs at runtime resolves relative to its working
//! directory, so the app keeps a single per-user home (`%LOCALAPPDATA%\Mite`),
//! launches the CLI with that as the CWD, and lays the deps out where the CLI
//! already expects them (`models\`, `cache\engines\`, `.gpu-runtime\bin\`).

use std::path::PathBuf;

use anyhow::{Context, Result};

pub const HOME_DIR_NAME: &str = "Mite";

/// `%LOCALAPPDATA%\Mite`. Does not create anything.
pub fn mite_home() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(base).join(HOME_DIR_NAME))
}

/// `mite_home`, created if missing.
pub fn ensure_home() -> Result<PathBuf> {
    let home = mite_home()?;
    std::fs::create_dir_all(&home)?;
    Ok(home)
}

/// The installed CLI binary: `<home>\bin\mite.exe`.
pub fn cli_exe() -> Result<PathBuf> {
    Ok(mite_home()?.join("bin").join("mite.exe"))
}

/// The GPU runtime DLL directory the CLI looks for via `MITE_GPU_RUNTIME_DIR`.
pub fn gpu_runtime_dir() -> Result<PathBuf> {
    Ok(mite_home()?.join(".gpu-runtime").join("bin"))
}

/// Per-run CLI log directory.
pub fn logs_dir() -> Result<PathBuf> {
    Ok(mite_home()?.join("logs"))
}

/// The model/dictionary files the OCR + lookup core needs to run at all.
const CORE_MODEL_FILES: &[&str] = &[
    "models/pp-ocrv5-mobile-det.onnx",
    "models/pp-ocrv5-mobile-rec.onnx",
    "models/pp-ocrv5-dict.txt",
    "models/jmdict-eng.json",
];

/// True when every core model/dictionary file is present in the home.
pub fn models_ready() -> bool {
    let Ok(home) = mite_home() else {
        return false;
    };
    CORE_MODEL_FILES.iter().all(|rel| home.join(rel).exists())
}

/// True when the optional GPU acceleration pack has been installed. Checks one
/// TensorRT and one CUDA DLL as a cheap proxy for the full set.
pub fn gpu_pack_installed() -> bool {
    let Ok(dir) = gpu_runtime_dir() else {
        return false;
    };
    dir.join("nvinfer_10.dll").exists() && dir.join("cudart64_12.dll").exists()
}
