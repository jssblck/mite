fn main() {
    emit_version();
    tauri_build::build()
}

/// Bake the build-time app version into `APP_VERSION`, mirroring the mite CLI's
/// tag-based scheme: an explicit `MITE_VERSION` override (set by release CI to
/// the release tag), then `git describe`, then the Cargo.toml version.
fn emit_version() {
    println!("cargo:rerun-if-env-changed=MITE_VERSION");
    let version = std::env::var("MITE_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=APP_VERSION={}", sanitize_version(&version));
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
