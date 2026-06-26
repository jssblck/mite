#!/usr/bin/env bash
#
# .claude/cloud-setup.sh: the cloud "Setup script" half of the Claude Code
# environment setup for mite.
#
# WHAT THIS IS
#   The toolchain-install half of bootstrapping a Claude Code on the web session.
#   It installs tools the cloud base image does NOT ship but mite's dev loop
#   needs. Wire it into the cloud environment's "Setup script" field with a
#   one-line guarded bootstrap:
#
#       if [ -f .claude/cloud-setup.sh ]; then bash .claude/cloud-setup.sh; fi
#
#   The Setup script runs as root before Claude Code launches, and may re-run on
#   any fresh session (after you change the script or the network allowlist, and
#   periodically), so every step is idempotent and a fast skip on the no-op path.
#   Cross-platform, per-session work (crate/dep fetch) lives in bootstrap.mjs.
#
# SCOPE
#   Cloud only, Ubuntu 24.04, root. If invoked somewhere without apt (e.g. a
#   curious local run on macOS/Windows), it no-ops rather than erroring.
#
# WHY THIS STAYS SMALL FOR MITE (Windows-first caveat)
#   mite is a Windows-first project: the capture/overlay path depends on the
#   `windows` crate and `xcap` (Direct3D / Windows Graphics Capture), and CI runs
#   on windows-latest. A full `cargo build` will NOT compile on a Linux cloud
#   container, and the heavy dep tree (ort, aws-lc, lindera, image) would also
#   risk the setup-cache timeout. So this script only installs missing tooling;
#   it does not build. The cross-platform lookup core can be built in-session with
#   `cargo build --lib`, and crate/dep fetch happens per session in bootstrap.mjs.
#   The GPU/model runtime and the private `eval` submodule are intentionally not
#   fetched here (no NVIDIA binaries, no third-party IP data on a cloud box).
#
# WHAT THE BASE IMAGE ALREADY SHIPS (do not reinstall)
#   rust, node, python+uv, go, ruby, a JVM, docker+compose, the postgres client,
#   redis, and the language registries. mite's genuine gaps are `gh` (PR/issue
#   flow) and `bun` (the app/ frontend package manager; site/ uses the bundled
#   npm). Both are pulled from GitHub release assets, which are on the default
#   Trusted network allowlist.

set -euo pipefail

log() { printf '[cloud-setup] %s\n' "$*"; }

# Ubuntu/apt only. Anywhere else, do nothing.
if ! command -v apt-get >/dev/null 2>&1; then
  log "apt-get not found; this script targets the Ubuntu cloud image only. Skipping."
  exit 0
fi

# Map uname -> the arch slugs release artifacts use.
case "$(uname -m)" in
  x86_64)  GH_ARCH=amd64; BUN_ARCH=x64 ;;
  aarch64) GH_ARCH=arm64; BUN_ARCH=aarch64 ;;
  *)       GH_ARCH=""; BUN_ARCH="" ;;
esac

# --- gh (GitHub CLI, from the published release tarball) --------------------
# The base image is most consistently missing gh, and mite's PR/issue flow uses
# it. We pull the release asset from github.com (Trusted allowlist) rather than
# the cli.github.com apt repo (not allowlisted). Reads GH_TOKEN if you set one.
if command -v gh >/dev/null 2>&1; then
  log "gh already present ($(gh --version | head -1)); skipping."
elif [ -z "$GH_ARCH" ]; then
  log "unsupported arch '$(uname -m)' for the gh release tarball; skipping gh."
else
  log "installing gh from its GitHub release..."
  # Resolve the latest tag by buffering the JSON body first, THEN grepping it.
  # `curl ... | grep -m1` is a race under `set -o pipefail`: grep exits on the
  # first match and closes the pipe while curl is still writing the body, so curl
  # dies on SIGPIPE (exit 23) and `set -e` turns that into a fatal abort.
  ghmeta="$(curl -fsSL https://api.github.com/repos/cli/cli/releases/latest)"
  ghver="$(printf '%s' "$ghmeta" | grep -m1 '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/')"
  if [ -z "$ghver" ]; then
    log "could not determine the latest gh release tag; skipping gh."
  else
    tmp="$(mktemp -d)"
    curl -fsSL -o "$tmp/gh.tar.gz" \
      "https://github.com/cli/cli/releases/download/v${ghver}/gh_${ghver}_linux_${GH_ARCH}.tar.gz"
    tar -xzf "$tmp/gh.tar.gz" -C "$tmp"
    install -m 0755 "$tmp/gh_${ghver}_linux_${GH_ARCH}/bin/gh" /usr/local/bin/gh
    rm -rf "$tmp"
    log "installed $(gh --version | head -1)"
  fi
fi

# --- bun (the app/ frontend package manager) --------------------------------
# CI installs app/ deps with `bun install --frozen-lockfile` and builds the
# frontend with `bun run build`; the base image does not ship bun. We install the
# standalone binary from its GitHub release (Trusted allowlist), unzipping with a
# tool apt provides if it is not already present.
if command -v bun >/dev/null 2>&1; then
  log "bun already present ($(bun --version)); skipping."
elif [ -z "$BUN_ARCH" ]; then
  log "unsupported arch '$(uname -m)' for the bun release; skipping bun."
else
  log "installing bun from its GitHub release..."
  if ! command -v unzip >/dev/null 2>&1; then
    apt-get update -qq && apt-get install -y -qq unzip
  fi
  # Buffer-then-grep for the same SIGPIPE reason as gh above.
  bunmeta="$(curl -fsSL https://api.github.com/repos/oven-sh/bun/releases/latest)"
  buntag="$(printf '%s' "$bunmeta" | grep -m1 '"tag_name"' | sed -E 's/.*"(bun-v[^"]+)".*/\1/')"
  if [ -z "$buntag" ]; then
    log "could not determine the latest bun release tag; skipping bun."
  else
    tmp="$(mktemp -d)"
    curl -fsSL -o "$tmp/bun.zip" \
      "https://github.com/oven-sh/bun/releases/download/${buntag}/bun-linux-${BUN_ARCH}.zip"
    unzip -q "$tmp/bun.zip" -d "$tmp"
    install -m 0755 "$tmp/bun-linux-${BUN_ARCH}/bun" /usr/local/bin/bun
    rm -rf "$tmp"
    log "installed bun $(bun --version)"
  fi
fi

log "done."
