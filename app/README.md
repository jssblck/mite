# Mite desktop app

A small Tauri desktop app that makes Mite installable and usable without a
terminal. It manages the mite CLI for non-technical users: it installs and
updates the engine, downloads the recognition models, detects and guides the
user through installing NVIDIA's GPU runtime (it never installs those binaries
itself), runs diagnostics, lets you pick a window from a live preview grid, and
launches `mite watch`.

The core CLI is unchanged. This app is a manager and launcher around it.

## How it works

The app owns a single per-user directory, the "mite home", at
`%LOCALAPPDATA%\Mite`:

```
%LOCALAPPDATA%\Mite\
  bin\mite.exe          downloaded from the GitHub release feed
  mite.toml             default config (written via `mite init-config`)
  app-settings.json     runtime, watch, and automatic eval-capture settings
  models\               detector/recognizer/dict/JMdict/jpdb-freq
  cache\engines\        TensorRT engine cache (CLI writes on first GPU run)
  nvidia-runtime\       optional: pip-install NVIDIA wheels here (the app watches it)
  logs\                 per-run CLI output
```

Because the mite CLI resolves its config, models, and engine cache relative to
its working directory, the app launches the CLI with the mite home as the
current directory. It also prepends the directories where the user's NVIDIA
runtime was found (recorded by the guided setup) to `PATH` so the Windows loader
can resolve the DLLs, and passes the recorded tier as `--backend`. No CLI path
rewriting is needed.

The app downloads the CLI and models from the GitHub release feed published by
`.github/workflows/release.yml`: it reads `release.json` for versions and
checksums, fetches `mite.exe` as a release asset, and drives the model downloads
from the release's `model-manifest.json` (the same manifest, URLs, and SHA256s
that `scripts\bootstrap-dev.ps1` uses). It does not download the NVIDIA runtime:
that is user-installed (see below). See [docs/releases.md](../docs/releases.md).

The app and the engine update in lockstep, app-first: the app prompts to update
itself before anything else, and the engine it installs is the newest release
within the app's own caret/semver range (a `0.2.0` app takes engine `0.2.x` but
never `0.3.0`), not always-latest. When the installed engine is older than or
outside that range, the app reconciles it on startup with no prompt. See
[docs/releases.md](../docs/releases.md) for the full model.

Advanced watch settings can enable the CLI's automatic eval-capture mode. The
user chooses a root folder with the native folder picker, and each watched
window writes to a normalized title folder below it. For example, watching a
window titled `Grace's Game: Deluxe Edition` with `C:\work\mite\eval` selected
writes captures under `C:\work\mite\eval\grace-s-game-deluxe-edition\`.

## Engine warmup

The first engine build after an install, update, or GPU-tier change compiles
TensorRT engines, which takes minutes; buried inside the first watch it reads as
"watching does nothing". So the app runs `mite warmup --json` up front: once per
launch after the engine reconcile settles, after a manual engine update or
config reset in Settings, and after the guided GPU setup closes (a tier change
means different engines). Warmup builds and warms exactly the sessions `watch`
will use, for whichever backend is recorded (TensorRT, CUDA, or CPU); on an
ordinary launch it is a seconds-long cache check.

While it runs, a banner shows step-by-step progress with an indeterminate bar
(TensorRT compilation reports no percentage), and the Watch tab and start
buttons are disabled; once a step reports a real from-scratch compile, the
banner explains the one-time multi-minute wait. The backend refuses to start a
watch during warmup and vice versa, since both would race the same engine
cache. A warmup failure never blocks watching: watch performs the same
preparation itself at startup, so the error banner just offers a retry. stderr
is mirrored to `logs\warmup-<pid>.log`.

## NVIDIA runtime setup

Mite's GPU pipeline needs NVIDIA runtime libraries (TensorRT, the CUDA runtime,
NVRTC, cuBLAS, cuDNN). NVIDIA's license does not let the app download, host,
bundle, or install any NVIDIA binary, so it cannot install them for the user.
Instead it detects what is installed, guides the user to install the missing
pieces themselves from NVIDIA, and records the result so it can launch the CLI
with the right options.

The flow:

1. After the core install (CLI, config, models), the app runs `mite doctor
   --json`, which probes for an NVIDIA GPU (`nvidia-smi`) and searches the system
   for the required DLLs (`PATH`, the CUDA Toolkit, TensorRT/cuDNN install
   locations, pip wheel layouts, and `MITE_GPU_RUNTIME_EXTRA_DIRS`). With no
   NVIDIA GPU it records the CPU tier and never nags.
2. If an NVIDIA GPU is present but the runtime is incomplete, the app opens a
   guided screen that shows a compact per-tier status (TensorRT and CUDA, each
   ready or incomplete), explains in plain language that these are NVIDIA's
   software and their license does not let Mite install them for the user, and
   offers two tabbed install routes from NVIDIA: the official download pages
   (`developer.nvidia.com`; cuDNN and TensorRT need a free NVIDIA developer
   account) or the official wheels (a copy-paste `pip install --target` command
   with the exact version pins, installing into the watched `nvidia-runtime\`
   folder). The pip route installs `tensorrt-cu12-libs` (the runtime DLLs that
   ORT loads natively) rather than the `tensorrt-cu12` meta-package, and uses
   `--extra-index-url https://pypi.nvidia.com` because the TensorRT runtime wheel
   is hosted on NVIDIA's index.
3. Skipping is offered above the status (GPU acceleration is optional) and opens
   a confirmation that explains the consequence based on what is already present:
   CPU if nothing NVIDIA is installed (much slower, always works), or CUDA-only
   if the CUDA tier is present but TensorRT is not (roughly 2x slower than
   TensorRT), and notes the setup can be re-run from Settings. While the screen
   is open it re-checks the dependencies every couple of seconds, so each tier
   flips to ready live as the user installs it; the "Continue" button enables
   once the TensorRT tier is complete.
4. Settings has a "Set up GPU acceleration" / "Re-run GPU setup" action that
   reopens this same detection-and-guidance flow later, for when the user
   installs or changes their NVIDIA runtime after first launch.
5. The detected tier (TensorRT, CUDA-only, or CPU) and the directories the DLLs
   were found in are recorded in `app-settings.json`. The launcher reads this to
   choose the `--backend` and the `PATH` the CLI is spawned with. The default
   `nvidia_tensor_rt_then_cuda` chain auto-degrades to CPU on its own, so the
   recorded backend is about clear UX (not implying TensorRT is active when only
   CUDA is present) rather than enabling the fallback.

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
bun run test           # vitest over the frontend's pure logic (e.g. the ANSI log parser)
bun run tauri build    # build the Windows installer (NSIS) under src-tauri\target\release\bundle
```

Backend checks (from `app\src-tauri`):

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

CI enforces these too. The app is a standalone crate (the repository root is not
a Cargo workspace), so the root Rust job does not see it; the separate `app` job
in `.github\workflows\ci.yml` builds the frontend (clippy needs `app\dist` for
`tauri::generate_context!`) and then runs the same fmt/test/clippy bar. The
crate carries the same `[lints.clippy]` table as the root crate so the standard
is identical on both surfaces.

## Versioning

The app version is derived from git tags at build time, the same way the CLI
does it: `app/src-tauri/build.rs` runs `git describe` into an `APP_VERSION` env,
and release CI overrides it with the release tag. One `v*` tag stream versions
the whole repo (CLI and app), published together in a single GitHub release.

## App self-update (signed, free)

The app updates itself with `tauri-plugin-updater`. This is separate from (and
takes priority over) the engine reconcile, which installs the mite CLI: the
updater here replaces the app shell. It uses Tauri's own minisign signature (not
Authenticode), which is free and unrelated to a code-signing certificate.

How it fits together:

- `tauri.conf.json` carries the updater `endpoints` (the release feed's
  `latest.json`) and the minisign **public** key.
- The release workflow signs each installer with the matching **private** key
  and publishes `latest.json` (version, notes, signed download URL) alongside the
  installer. The app polls that file on launch and, when a newer signed build
  exists, shows a priority banner ("Update Mite") that downloads the next
  installer, verifies it against the public key, installs it, and relaunches. The
  same control is also available on demand in Settings -> App version.
- The engine then follows: the relaunched app pulls the engine matching its new
  version's caret range automatically (see the lockstep model in
  [docs/releases.md](../docs/releases.md)). So the user-facing order is always
  app-first, engine-after, with only the app step prompted.

### One-time key setup

Generate the updater keypair once and keep the private key safe (losing it means
you can no longer ship updates that existing installs will accept):

```powershell
cd app
bun run tauri signer generate -w "$HOME\.tauri\mite-updater.key"
```

This writes the private key to `~/.tauri/mite-updater.key` (kept out of the repo;
`app/.gitignore` also blocks stray `*.key` files) and prints the public key. The
public key already lives in `tauri.conf.json` `plugins.updater.pubkey`; if you
rotate the key, replace it there.

Then add two GitHub Actions secrets so CI can sign releases:

- `TAURI_SIGNING_PRIVATE_KEY`: the **contents** of `~/.tauri/mite-updater.key`.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the password you set (empty if none).

Until those secrets exist, the release job still builds, but it produces an
unsigned installer with no `latest.json`, so self-update is simply unavailable
for that release (logged as a warning, not a failure). Building the installer
locally with `bun run tauri build` needs the same `TAURI_SIGNING_PRIVATE_KEY`
env set, or pass `--config '{"bundle":{"createUpdaterArtifacts":false}}'` to
build an unsigned installer.

## Code signing (Authenticode) is optional and not enabled

The installer is **not** Authenticode code-signed, because that needs a paid
certificate from a certificate authority (an updater key, above, is a different
and free thing). The practical consequence: on first run users see a Windows
SmartScreen "Windows protected your PC / unknown publisher" prompt and have to
click **More info -> Run anyway**. The app still installs and works normally.

To enable it later, sign through Tauri's own bundling rather than a separate
`signtool` step. The updater signature must be computed over the *final, signed*
installer, and Tauri signs before generating that signature; signing the
installer after the fact would invalidate the updater signature. Set
`bundle.windows.certificateThumbprint` (or a `signCommand`) in `tauri.conf.json`,
supply the certificate to CI as secrets, and the same release flow produces a
signed, self-updatable installer. EV certificates also clear the SmartScreen
prompt immediately; OV certificates clear it after some download reputation
accrues.
