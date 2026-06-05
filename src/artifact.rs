//! Small shared helpers for writing on-disk artifacts (reports, manifests,
//! debug captures) and for the timing/timestamp values that go in them.
//!
//! These were previously copy-pasted across `capture_report`, `session_collect`,
//! `debug_capture`, `lookup`, `eval`, and `main`; centralizing them keeps the
//! `artifact_version`/pretty-JSON/`create_dir_all` convention in one place.

use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

/// Schema version stamped into on-disk artifacts (eval reports, debug
/// captures). Bump when the JSON shape changes in a way downstream tools care
/// about.
pub const ARTIFACT_VERSION: u32 = 1;

/// Wall-clock time since the Unix epoch, in milliseconds. Used to stamp and
/// name artifacts; clamps to 0 if the system clock is before the epoch.
pub fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Milliseconds elapsed since `start`, as a float (for sub-millisecond stage
/// timings in reports).
pub fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Serialize `value` as pretty JSON to `path`, creating the parent directory if
/// it does not yet exist.
pub fn write_json_pretty(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}
