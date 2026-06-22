//! HTTP download, checksum verification, and zip extraction helpers.
//!
//! These run on a blocking thread (driven from async commands via
//! `spawn_blocking`), using `ureq` for simple synchronous IO with a progress
//! callback so the UI can show real bytes-received counts.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// User-Agent sent on every request. GitHub's API rejects requests without one.
pub const USER_AGENT: &str = concat!("mite-app/", env!("APP_VERSION"));

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .build()
}

/// GET a URL and deserialize the JSON body.
pub fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let resp = agent()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/json")
        .call()
        .with_context(|| format!("GET {url}"))?;
    resp.into_json::<T>()
        .with_context(|| format!("decoding JSON from {url}"))
}

/// Download `url` to `dest`, calling `on_progress(received, total)` as bytes
/// arrive (`total` is 0 when the server sends no Content-Length). Writes to a
/// sibling `.part` file first and renames into place so a partial download
/// never looks complete.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<()> {
    let resp = agent()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;

    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut received = 0u64;
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buf[..read])?;
        received += read as u64;
        on_progress(received, total);
    }
    drop(file);
    // Rename can fail if the destination is open elsewhere; remove first.
    let _ = fs::remove_file(dest);
    fs::rename(&tmp, dest).with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

/// Hex-encoded SHA256 of a file.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Fail unless the file's SHA256 matches `expected` (case-insensitive hex).
pub fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

/// Extract every entry of a zip into `dest_dir`, preserving internal structure.
pub fn extract_zip_all(zip_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    fs::create_dir_all(dest_dir)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest_dir.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out_file = fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut out_file)?;
    }
    Ok(())
}

/// Extract the first zip member whose base name matches `member_glob` (a simple
/// single-`*` pattern, e.g. `jmdict-eng-*.json`) into `dest_file`.
pub fn extract_zip_member(zip_path: &Path, member_glob: &str, dest_file: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        let base = name.rsplit(['/', '\\']).next().unwrap_or(&name);
        if glob_match(member_glob, base) {
            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = fs::File::create(dest_file)?;
            std::io::copy(&mut entry, &mut out_file)?;
            return Ok(());
        }
    }
    bail!("no member matched {member_glob} in {}", zip_path.display());
}

/// Match a name against a pattern containing at most one `*` wildcard.
fn glob_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, suffix)) => {
            name.len() >= prefix.len() + suffix.len()
                && name.starts_with(prefix)
                && name.ends_with(suffix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn glob_matches_single_wildcard() {
        assert!(glob_match("jmdict-eng-*.json", "jmdict-eng-3.6.2.json"));
        assert!(glob_match("*.json", "anything.json"));
        assert!(glob_match("exact.txt", "exact.txt"));
        assert!(!glob_match(
            "jmdict-eng-*.json",
            "jmdict-eng-3.6.2.json.bak"
        ));
        assert!(!glob_match("a-*.json", "b-x.json"));
    }
}
