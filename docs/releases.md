# Releases

This page explains how mite is versioned, how to cut a release, what each
published asset is for, and how the desktop app consumes the release feed.

## Versioning

Versions come from git tags shaped like `vX.Y.Z` (for example `v0.1.0`).

The version that a binary reports through `--version` is resolved at build time
by `build.rs` (see `src/version.rs`), with this precedence:

1. An explicit `MITE_VERSION` environment override. Release CI sets this to the
   release tag so the reported version is exact regardless of clone depth.
2. Otherwise `git describe` (the nearest reachable tag, or a short commit string
   when no tag is reachable).

`Cargo.toml` keeps a `0.0.0` placeholder on purpose: it is never the source of
truth for the released version. Do not bump it to mark a release; tag instead.

## Cutting a release

1. Make sure `main` is in the state you want to ship.
2. Create and push a version tag:

   ```powershell
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. The `Release` workflow builds the CLI and (when the desktop app is buildable)
   the installer, then creates a DRAFT GitHub Release with notes generated from
   the pull requests merged since the previous tag. No NVIDIA GPU runtime asset
   is built or published: that runtime is user-installed (see below).
4. Review the draft release, edit the generated notes if needed, then publish it
   by hand. A bare `X.Y.Z` tag is published as the latest stable release; any
   other tag shape (for example a pre-release suffix) is marked as a prerelease.

## Published assets

Each release attaches the following assets:

- `mite.exe`: the Windows CLI. Built with `MITE_VERSION` pinned to the tag.
- `onnxruntime_providers_shared.dll`, `onnxruntime_providers_cuda.dll`,
  `onnxruntime_providers_tensorrt.dll`: ONNX Runtime's provider bridge DLLs
  (ONNX Runtime is MIT-licensed). The `ort` build emits them next to `mite.exe`,
  and ONNX Runtime loads them from there to register the CUDA/TensorRT execution
  providers, so they are installed alongside the engine. Without them the engine
  cannot reach the GPU and silently runs on the CPU. These are the only GPU-path
  binaries Mite ships; the NVIDIA runtime is still user-installed (see below).
- `model-manifest.json`: the repo's model manifest, copied as-is. It lists the
  OCR models, dictionaries, and frequency data with their download URLs and
  checksums, so the app can fetch and verify model files.
- `release.json`: the feed the desktop app polls to update the mite CLI (see
  below).
- `latest.json`: the Tauri updater feed the desktop app uses to update **itself**
  (see below). Present only when the installer was signed.
- `SHA256SUMS`: a sha256 plus filename line for every published asset, for
  manual verification.
- The desktop app installer (NSIS `.exe` and/or MSI `.msi`). The app build is a
  required job: if it fails, the release (and every PR dry run) fails, so a
  release never ships without an installer.

The NVIDIA GPU runtime libraries (TensorRT, the CUDA runtime, NVRTC, cuBLAS,
cuDNN) are deliberately not among these assets. Mite never redistributes NVIDIA
binaries: the user installs them directly from NVIDIA, and the desktop app
detects what is present and guides the rest. See
[app/README.md](../app/README.md) and `THIRD_PARTY_NOTICES.md`.

## How the desktop app consumes release.json

The Tauri desktop app polls `release.json` to discover updates. Its shape is:

```json
{
  "version": "v0.1.0",
  "cli": {
    "asset": "mite.exe",
    "sha256": "<hex>",
    "extraFiles": [
      { "asset": "onnxruntime_providers_shared.dll", "sha256": "<hex>" },
      { "asset": "onnxruntime_providers_cuda.dll", "sha256": "<hex>" },
      { "asset": "onnxruntime_providers_tensorrt.dll", "sha256": "<hex>" }
    ]
  },
  "modelManifest": { "asset": "model-manifest.json" },
  "installer": { "asset": "<installer filename>", "sha256": "<hex>" }
}
```

`version` is the release tag. Each entry names an asset attached to the same
release and (where a checksum is meaningful) its sha256. The app resolves which
release to read (see lockstep below), downloads the named assets from that
GitHub Release, and verifies each download against the listed sha256. The
`installer` entry is always present: the desktop app build is a required release
job, so every release ships an installer.

`cli.extraFiles` lists the engine sidecars the app installs into `bin\` next to
`mite.exe`: ONNX Runtime's provider bridge DLLs, which the engine needs to
register a GPU execution provider. The field is additive and defaults to empty,
so an older `release.json` without it still parses (the app then installs just
the exe, as before). Each entry is verified against its sha256 like any other
asset.

## How the app and CLI stay in lockstep

The app shell and the CLI engine ship in the same release, but they update on two
clocks, and the app's clock leads:

1. **The app updates itself first.** On launch the app polls `latest.json` (the
   Tauri updater feed) and, when a newer signed build exists, shows a priority
   banner with a single "Update Mite" action that downloads, verifies, installs,
   and relaunches. Nothing about the app self-update is silent: it is always a
   prompt.
2. **The engine follows the app, automatically.** A given app build does not pull
   "latest"; it pulls the newest engine within its own caret/semver range. For a
   `0.x` app that is the same `0.MINOR` line (so a `0.2.0` app accepts engine
   `0.2.1` but not `0.3.0`); for `>=1.0` it is the same major. The app resolves
   that by listing releases and picking the newest non-draft, non-prerelease tag
   that satisfies the range, then reading that release's `release.json`. If the
   installed engine is older than (or outside) that range, the app downloads the
   matching engine on startup with no prompt, because which engine is correct is
   a consequence of which app version is running.

   If no release satisfies the range, the app falls back to the release tagged
   exactly its own version. This is what lets a prerelease app reach its matching
   prerelease engine: prereleases are skipped by the rules above, so a
   `0.3.0-rc.1` app would otherwise find nothing and instead pins to the
   `v0.3.0-rc.1` engine. (An untagged `0.0.0` local build has no pin at all and
   falls back to the latest release so dev installs still work.)

Together these mean an old app never eagerly jumps to a breaking new engine: it
updates itself first, and the relaunched app then reconciles the engine to the
range it understands. The model files are versioned with the engine, so they are
fetched from the same resolved release. An untagged local build (the `0.0.0`
placeholder) has no caret pin and falls back to the latest release so dev installs
still work.

## How the desktop app updates itself (latest.json)

The CLI and the app update along two separate paths. `release.json` (above)
drives updating the mite **CLI** in place. The app **shell** updates itself with
`tauri-plugin-updater`, which polls `latest.json`:

```json
{
  "version": "0.1.0",
  "notes": "See the release notes at https://github.com/<owner>/mite/releases/tag/v0.1.0",
  "pub_date": "2026-01-01T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<minisign signature of the installer>",
      "url": "https://github.com/<owner>/mite/releases/download/v0.1.0/Mite_0.1.0_x64-setup.exe"
    }
  }
}
```

The app compares `version` against the version baked into the running build
(CI stamps `tauri.conf.json` from the tag), downloads the installer at `url`,
verifies it against the minisign **public** key compiled into the app, installs
it, and relaunches. The release workflow generates `latest.json` from the signed
installer and its detached `.sig`; the bare `.sig` is not published separately
because the signature is embedded here.

### Signing secrets

`latest.json` (and a self-updatable installer) is produced only when the release
build can sign the installer with the updater key. Configure two repository
secrets:

- `TAURI_SIGNING_PRIVATE_KEY`: the contents of the minisign private key generated
  by `bun run tauri signer generate`.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: that key's password (empty if none).

Without these the release still succeeds, but the installer is unsigned and no
`latest.json` is published, so app self-update is unavailable for that release.
This updater signing is free and is independent of Authenticode installer
signing, which is optional and still not enabled. See
[app/README.md](../app/README.md) for the key setup and the SmartScreen note.

## Dry run

The `Release` workflow runs as a dry run whenever it is not triggered by a
version tag:

- On every pull request (so a change that breaks the release build fails on the
  PR, not when a tag is cut).
- On every push to `main`.
- On a manual `workflow_dispatch` from the Actions tab.

A dry run builds and packages every asset, generates `release.json`,
`latest.json` (when signing is configured), and `SHA256SUMS`, and writes a job
summary, but it does not create a GitHub Release. The only thing that turns a dry
run into a real release is the trigger being a `vX.Y.Z` tag. On a non-tag ref the
version is derived from `git describe` and treated as a prerelease.

Site-only pull requests (every changed file under `site/**`) skip the release
dry run; the site has its own deploy workflow and never affects release
artifacts. (main pushes and tags always build, so a release is never silently
skipped.)

The skip happens at the job level, not by a workflow path filter, and that
distinction matters because `Build CLI`, `Build desktop app installer`, and
`Release` are required status checks on `main`. A workflow that a path filter
prevents from starting never reports those checks, so they sit pending forever
and block the PR. Instead the workflow always starts and a cheap `changes` job
diffs the PR; on a site-only PR the build and release jobs are skipped via an
`if` condition, which reports a `skipped` conclusion that GitHub counts as a
passing required check. The `changes` job defaults to building on anything that
is not a clearly site-only PR (including any error computing the diff), so the
gate is never bypassed by accident.

### Build caching

The CLI and app builds are cached with `Swatinem/rust-cache` (and a Bun module
cache for the app's frontend deps). Caches are written only by `main` and tag
builds and restored read-only by pull requests, because GitHub only lets a PR
restore caches its base branch populated. That is why the release workflow also
runs on `main`: those runs keep the shared-key caches warm so PR dry runs reuse
them instead of compiling from scratch.
