#!/usr/bin/env bash
set -euo pipefail

# setup-claude-env.sh
#
# Provision a fresh Claude Code cloud environment for mite.
# Targets a Debian/Ubuntu Linux container that starts with nothing installed.
# Idempotent: safe to re-run. Invoke as: ./scripts/setup-claude-env.sh
#
# IMPORTANT: mite is a Windows-first project. The capture/overlay path depends
# on the `windows` crate and `xcap` (Direct3D / Windows Graphics Capture), and
# CI runs on windows-latest. On a Linux container the lookup/segmentation core
# may build, but a full `cargo build` can fail on the Windows-only crates. This
# script installs the toolchain and dependencies and attempts a build, but it
# treats build failure as a warning rather than aborting, so the environment
# still provisions. The GPU/model runtime (TensorRT/CUDA) is never fetched here;
# unit tests and the lookup core run without a GPU or models.
#
# What it does:
#   - installs build tooling (git, build-essential, pkg-config, libssl-dev)
#   - installs the Rust stable toolchain
#   - creates the runtime model/cache directories
#   - fetches dependencies and attempts to warm the build (non-fatal)
#
# Optional, not done here (needs SSH + Git LFS + real data):
#   - eval submodule:  git submodule update --init eval
#   - OCR models:       see model-manifest.json / docs

GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
log()  { printf "${GREEN}==>${NC} %s\n" "$1"; }
warn() { printf "${YELLOW}warn:${NC} %s\n" "$1" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

SUDO=""
if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi

apt_install() {
  if ! command -v apt-get >/dev/null 2>&1; then
    warn "apt-get not found; please install manually: $*"
    return 0
  fi
  $SUDO apt-get update -y
  $SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@"
}

log "Installing system build dependencies"
apt_install git build-essential pkg-config libssl-dev ca-certificates curl

if ! command -v cargo >/dev/null 2>&1; then
  log "Installing Rust (stable)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal
fi
# shellcheck disable=SC1091
. "$HOME/.cargo/env"
rustup component add clippy rustfmt >/dev/null 2>&1 || warn "could not add clippy/rustfmt"

log "Creating runtime directories (models/, cache/engines/)"
mkdir -p models cache/engines

log "Fetching dependencies"
cargo fetch --locked || cargo fetch

log "Attempting to warm the build (non-fatal: mite is Windows-first)"
if cargo build --all-targets; then
  log "Build succeeded on this platform"
else
  warn "cargo build did not complete on Linux. This is expected if the Windows-only"
  warn "crates (windows, xcap) are reached. mite is intended for local Windows dev;"
  warn "the lookup/segmentation core and its unit tests may still build and run."
fi

log "mite environment ready (see notes above for GPU/model and eval-data setup)"
