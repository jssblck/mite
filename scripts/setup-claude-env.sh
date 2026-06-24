#!/usr/bin/env bash
# Claude Code cloud environment setup for mite.
#
# Runs as root on Ubuntu 24.04 before the session starts, per
# https://code.claude.com/docs/en/claude-code-on-the-web#setup-scripts
# Point an environment's Setup script at:  bash scripts/setup-claude-env.sh
#
# IMPORTANT: mite is a Windows-first project. The capture/overlay path depends on
# the `windows` crate and `xcap` (Direct3D / Windows Graphics Capture), and CI
# runs on windows-latest. A full `cargo build` will likely NOT compile on a Linux
# cloud container. This script therefore only fetches crates (so the lookup core
# can be built in-session) and does NOT run a full build, which would also risk
# the ~5-minute setup-cache timeout given mite's large dependency tree (ort,
# aws-lc, lindera, image). The GPU/model runtime and the private eval submodule
# are intentionally not fetched.
# Idempotent and cached; safe to re-run.

set -uo pipefail

log()  { printf '==> %s\n' "$1"; }
warn() { printf 'warn: %s\n' "$1" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

persist_path() {
  [ -n "${CLAUDE_ENV_FILE:-}" ] || return 0
  printf 'export PATH="%s:$PATH"\n' "$1" >> "$CLAUDE_ENV_FILE"
}

if ! command -v cargo >/dev/null 2>&1; then
  log "cargo not found; installing Rust via rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal || warn "rustup install failed"
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env" 2>/dev/null || true
  persist_path "$HOME/.cargo/bin"
fi
command -v rustup >/dev/null 2>&1 && { rustup component add clippy rustfmt >/dev/null 2>&1 || true; }

log "Creating runtime directories (models/, cache/engines/)"
mkdir -p models cache/engines || true

log "Fetching crates (the lookup core can then be built in-session)"
cargo fetch --locked || cargo fetch || warn "cargo fetch failed (check the environment's network access level)"

warn "Skipping 'cargo build': mite is Windows-first, so a full Linux build will likely"
warn "fail on the windows/xcap crates, and the heavy dep tree risks the setup timeout."
warn "Build the platform-independent lookup core in-session, e.g. 'cargo build --lib'."

log "mite environment ready (crates fetched; see notes above for GPU/model and eval-data setup)"
exit 0
