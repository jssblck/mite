use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_DLLS: &[&str] = &[
    "nvinfer_10.dll",
    "nvonnxparser_10.dll",
    "nvinfer_plugin_10.dll",
    "cudart64_12.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cudnn64_9.dll",
    "cudnn_ops64_9.dll",
    "cudnn_cnn64_9.dll",
];

fn main() {
    emit_version();

    println!("cargo:rerun-if-env-changed=MITE_SKIP_GPU_RUNTIME_STAGE");
    println!("cargo:rerun-if-env-changed=MITE_GPU_RUNTIME_DIR");
    println!("cargo:rerun-if-changed=.gpu-runtime/bin");

    if env::var_os("MITE_SKIP_GPU_RUNTIME_STAGE").is_some() || !cfg!(target_os = "windows") {
        return;
    }

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set by Cargo"),
    );
    let runtime_bin = env::var_os("MITE_GPU_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join(".gpu-runtime").join("bin"));
    if !runtime_bin.is_dir() {
        warn(format!(
            "GPU runtime cache is missing at {}; run scripts\\install-gpu-runtime.ps1 to enable TensorRT/CUDA staging",
            runtime_bin.display()
        ));
        return;
    }

    let dlls = match runtime_dlls(&runtime_bin) {
        Ok(dlls) => dlls,
        Err(error) => {
            warn(format!(
                "failed to scan GPU runtime cache {}: {error}",
                runtime_bin.display()
            ));
            return;
        }
    };

    if dlls.is_empty() {
        warn(format!(
            "GPU runtime cache {} contains no DLLs; run scripts\\install-gpu-runtime.ps1",
            runtime_bin.display()
        ));
        return;
    }

    let missing = missing_required_dlls(&runtime_bin);
    if !missing.is_empty() {
        warn(format!(
            "GPU runtime cache is missing required DLLs: {}; rerun scripts\\install-gpu-runtime.ps1",
            missing.join(", ")
        ));
    }

    let Some(profile_dir) = cargo_profile_dir() else {
        warn("could not infer Cargo profile output dir; GPU runtime DLLs were not staged");
        return;
    };

    for dst in [
        profile_dir.clone(),
        profile_dir.join("deps"),
        profile_dir.join("examples"),
    ] {
        if let Err(error) = fs::create_dir_all(&dst) {
            warn(format!("failed to create {}: {error}", dst.display()));
            continue;
        }
        for dll in &dlls {
            if let Err(error) = copy_if_stale(dll, &dst.join(dll.file_name().unwrap())) {
                warn(format!(
                    "failed to stage {} into {}: {error}",
                    dll.display(),
                    dst.display()
                ));
            }
        }
    }
}

/// Bake the build-time version into `MITE_VERSION` for `src/version.rs`.
///
/// Precedence: an explicit `MITE_VERSION` override (set by release CI to the
/// release tag), then `git describe` (a reachable tag, otherwise the short
/// commit, with a `-dirty` suffix for uncommitted changes), then the
/// `Cargo.toml` version as a last resort when git is unavailable (for example a
/// source-tarball build with no `.git`).
fn emit_version() {
    println!("cargo:rerun-if-env-changed=MITE_VERSION");
    let version = env::var("MITE_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!(
        "cargo:rustc-env=MITE_VERSION={}",
        sanitize_version(&version)
    );
}

fn git_describe() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--always", "--tags", "--dirty=-dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Keep the reported version to a predictable, printable character set so a
/// stray ref name can never inject control characters into `--version` output.
fn sanitize_version(raw: &str) -> String {
    let mut version: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if version.is_empty() {
        version = env!("CARGO_PKG_VERSION").to_string();
    }
    version.truncate(128);
    version
}

fn runtime_dlls(runtime_bin: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dlls = Vec::new();
    for entry in fs::read_dir(runtime_bin)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            dlls.push(path);
        }
    }
    dlls.sort();
    Ok(dlls)
}

fn missing_required_dlls(runtime_bin: &Path) -> Vec<String> {
    REQUIRED_DLLS
        .iter()
        .copied()
        .filter(|name| !runtime_bin.join(name).is_file())
        .map(str::to_string)
        .collect()
}

fn cargo_profile_dir() -> Option<PathBuf> {
    let profile = env::var("PROFILE").ok()?;
    let out_dir = PathBuf::from(env::var_os("OUT_DIR")?);
    out_dir
        .ancestors()
        .find(|path| path.file_name().and_then(OsStr::to_str) == Some(profile.as_str()))
        .map(Path::to_path_buf)
}

fn copy_if_stale(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !is_stale(src, dst)? {
        return Ok(());
    }
    fs::copy(src, dst)?;
    Ok(())
}

fn is_stale(src: &Path, dst: &Path) -> std::io::Result<bool> {
    let src_meta = fs::metadata(src)?;
    let Ok(dst_meta) = fs::metadata(dst) else {
        return Ok(true);
    };
    if src_meta.len() != dst_meta.len() {
        return Ok(true);
    }
    match (src_meta.modified(), dst_meta.modified()) {
        (Ok(src_modified), Ok(dst_modified)) => Ok(src_modified > dst_modified),
        _ => Ok(false),
    }
}

fn warn(message: impl AsRef<str>) {
    println!("cargo:warning={}", message.as_ref());
}
