# Mite desktop app

A small Tauri desktop app that makes Mite installable and usable without a
terminal. It manages the mite CLI for non-technical users: it installs and
updates the engine, downloads the recognition models and the optional GPU
acceleration pack, runs diagnostics, lets you pick a window from a live preview
grid, and launches `mite watch`.

The core CLI is unchanged. This app is a manager and launcher around it.

## How it works

The app owns a single per-user directory, the "mite home", at
`%LOCALAPPDATA%\Mite`:

```
%LOCALAPPDATA%\Mite\
  bin\mite.exe          downloaded from the GitHub release feed
  mite.toml             default config (written via `mite init-config`)
  models\               detector/recognizer/dict/JMdict/jpdb-freq
  cache\engines\        TensorRT engine cache (CLI writes on first GPU run)
  .gpu-runtime\bin\      optional GPU DLLs (TensorRT/CUDA/cuDNN)
  logs\                 per-run CLI output
```

Because the mite CLI resolves its config, models, and engine cache relative to
its working directory, the app launches the CLI with the mite home as the
current directory and exports `MITE_GPU_RUNTIME_DIR` (also prepended to `PATH`
so the Windows loader can find the GPU DLLs). No CLI path rewriting is needed.

The app downloads everything from the GitHub release feed published by
`.github/workflows/release.yml`: it reads `release.json` for versions and
checksums, fetches `mite.exe` and the GPU pack as release assets, and drives the
model downloads from the release's `model-manifest.json` (the same manifest, URLs,
and SHA256s that `scripts\bootstrap-dev.ps1` uses). See
[docs/releases.md](../docs/releases.md).

## Shared design language

The app deliberately shares the marketing site's design system so the two feel
like one product. `app/src/main.tsx` imports `site/src/styles/global.css` (which
imports `tokens.css`) as the single source of truth for color, type, spacing,
and the button/label primitives; `app/src/styles/app.css` adds only
app-specific component styles on top of those tokens. The brand reticle
(`MiteMark`) and the part-of-speech color channel are reused as well. Vite's
`server.fs.allow` is widened to the repo root so the dev server can serve the
shared CSS from `../site`.

## Develop

Prerequisites: a recent Rust toolchain, [bun](https://bun.sh), and the
[Tauri v2 system prerequisites](https://tauri.app/start/prerequisites/) for
Windows (WebView2 is preinstalled on Windows 11).

```powershell
cd app
bun install
bun run tauri dev      # run the app against the live Rust backend
```

Other commands:

```powershell
bun run build          # type-check (tsc) and build the frontend bundle
bun run tauri build    # build the Windows installer (NSIS) under src-tauri\target\release\bundle
```

Backend checks (from `app\src-tauri`):

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Versioning

The app version is derived from git tags at build time, the same way the CLI
does it: `app/src-tauri/build.rs` runs `git describe` into an `APP_VERSION` env,
and release CI overrides it with the release tag. One `v*` tag stream versions
the whole repo (CLI and app), published together in a single GitHub release.

## Not done yet (needs signing keys)

Silent app self-update via `tauri-plugin-updater` is intentionally not wired up:
it requires a code-signing certificate and an updater signing key, which are the
maintainer's to provide. Today the app checks the release feed and surfaces a
"newer engine available" banner, and updates the CLI in place; updating the app
shell itself is done by downloading a new installer. Wiring the signed updater
(and Authenticode signing of the installer) is the documented follow-up: add the
signing secrets to CI, set `plugins.updater` in `tauri.conf.json` with the
public key, and register the updater plugin in `lib.rs`.
