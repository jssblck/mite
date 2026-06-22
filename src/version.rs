//! The version string reported by `mite --version`.

/// The build-time version, derived from git tags by `build.rs`.
///
/// This is a release tag when one is reachable, otherwise the short commit SHA,
/// with a `-dirty` suffix when the working tree had uncommitted changes at build
/// time. It falls back to the `Cargo.toml` version when git is unavailable. The
/// desktop app reads this string from `mite --version` to decide whether the
/// installed CLI is up to date with the release feed.
pub const VERSION: &str = env!("MITE_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_present() {
        // build.rs always emits a non-empty MITE_VERSION (git describe, an
        // override, or the Cargo.toml fallback), so this should never be blank.
        assert!(!VERSION.trim().is_empty());
    }
}
