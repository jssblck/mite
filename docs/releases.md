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

- `mite.exe`: the Windows CLI. Built with `MITE_VERSION` pinned to the tag and
  with the GPU runtime staging skipped, so the GPU DLLs do not get stapled to
  the exe.
- `model-manifest.json`: the repo's model manifest, copied as-is. It lists the
  OCR models, dictionaries, and frequency data with their download URLs and
  checksums, so the app can fetch and verify model files.
- `release.json`: the feed the desktop app polls to update the mite CLI (see
  below).
- `latest.json`: the Tauri updater feed the desktop app uses to update **itself**
  (see below). Present only when the installer was signed.
- `SHA256SUMS`: a sha256 plus filename line for every published asset, for
  manual verification.
- The desktop app installer (NSIS `.exe` and/or MSI `.msi`), when the app build
  succeeds. While the app is still in development its build is allowed to fail
  without blocking the rest of the release, so an installer may be absent from
  an early release.

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
  "cli": { "asset": "mite.exe", "sha256": "<hex>" },
  "modelManifest": { "asset": "model-manifest.json" },
  "installer": { "asset": "<installer filename>", "sha256": "<hex>" }
}
```

`version` is the release tag. Each entry names an asset attached to the same
release and (where a checksum is meaningful) its sha256. The app compares
`version` against the version it currently has, downloads the named assets from
the matching GitHub Release, and verifies each download against the listed
sha256. The `installer` entry is present only when an installer was built for
that release.

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

To exercise the pipeline without cutting a tag, run the `Release` workflow
manually from the Actions tab (a `workflow_dispatch`) with `dry_run` set to
true. It builds and packages every asset, generates `release.json` and
`SHA256SUMS`, and writes a job summary, but it does not create a GitHub Release.
On a non-tag ref the version is derived from `git describe` and treated as a
prerelease.
