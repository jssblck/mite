use std::env;

fn main() {
    emit_version();
}

/// Bake the build-time version into `MITE_VERSION` for `src/version.rs`.
///
/// Precedence: an explicit `MITE_VERSION` override (set by release CI to the
/// release tag), then `git describe` (a reachable tag, otherwise the short
/// commit, with a `-dirty` suffix for uncommitted changes), then the
/// `Cargo.toml` version as a last resort when git is unavailable (for example a
/// source-tarball build with no `.git`).
///
/// This build script deliberately does NOT touch the NVIDIA runtime. Mite never
/// downloads, hosts, bundles, or stages NVIDIA binaries: the runtime is
/// installed by the user (or, for development, the contributor) from NVIDIA, and
/// the OS loader resolves it from the standard search path. See
/// `docs/local-windows.md`.
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
