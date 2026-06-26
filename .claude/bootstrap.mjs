#!/usr/bin/env node
//
// .claude/bootstrap.mjs: the per-session half of mite's Claude Code env setup.
//
// WHAT THIS IS
//   The cheap, runs-every-time half of bootstrapping a session. Wired as a
//   SessionStart hook in .claude/settings.json, it runs on every session start
//   and resume, BOTH locally (Windows/macOS/Linux) and in Claude Code on the
//   web. The other half (cloud-setup.sh) does the cloud-only, root/apt toolchain
//   install before Claude Code launches; this does the fast, cross-platform
//   per-session prep.
//
//   Node specifically: Claude Code itself runs on Node, so `node` is guaranteed
//   present on every platform with no extra toolchain and no compile step. Keep
//   this file Node + standard library only: no dependencies, no build.
//
// STEP GATING
//   There is no blanket "cloud only" guard. Each step decides for itself whether
//   it applies, so the same file is correct everywhere:
//     - writeEnvVars() runs only in the cloud (needs $CLAUDE_ENV_FILE).
//     - installDeps()  fetches deps only for the manifests/managers present.
//   The script never fails a session: steps log and continue on error.
//
// MITE NOTES
//   mite is Windows-first. The capture/overlay path needs the `windows` crate and
//   xcap (Direct3D / WGC), so a full `cargo build` will not compile on a Linux
//   cloud box. installDeps() therefore only *fetches* crates and node deps; it
//   never builds. The cross-platform lookup core builds in-session with
//   `cargo build --lib`. The GPU/model runtime and the private `eval` submodule
//   are deliberately left alone (no NVIDIA binaries, no third-party IP data).

import { existsSync, appendFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

// Resolve the repo root from this file's own location (<root>/.claude/bootstrap.mjs)
// rather than from process.cwd(): SessionStart hooks normally run at the repo
// root but the platform does not strictly guarantee it, so anchoring on
// import.meta.url keeps every path below correct regardless of cwd.
const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));

const isCloud = process.env.CLAUDE_CODE_REMOTE === "true";
const isWin = process.platform === "win32";
const log = (msg) => console.log(`[bootstrap] ${msg}`);

// Does `tool` resolve on PATH? Use the platform's own lookup (`where` on Windows,
// `command -v` on POSIX) so Windows .cmd shims like npm.cmd are found: a plain
// execFile of "npm" with no shell would miss them and false-negative.
const has = (tool) => {
  try {
    execSync(`${isWin ? "where" : "command -v"} ${tool}`, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
};

// --- Step 1: env vars (cloud only) -----------------------------------------
// Cloud sessions persist env vars for later Bash tool calls by appending
// `export` lines to $CLAUDE_ENV_FILE. mite reads its runtime config from
// mite.toml and needs no non-secret env defaults, so this is currently a no-op
// placeholder: add NON-SECRET defaults here if that ever changes. Real secrets
// (e.g. GH_TOKEN) belong in the cloud environment object, never in git.
function writeEnvVars() {
  const envFile = process.env.CLAUDE_ENV_FILE;
  if (!isCloud || !envFile) {
    log("env vars: not a cloud session (no $CLAUDE_ENV_FILE); skipping.");
    return;
  }
  // No non-secret env defaults for mite today. Leave empty.
  const vars = {};
  const entries = Object.entries(vars);
  if (entries.length === 0) {
    log("env vars: none configured; skipping.");
    return;
  }
  const lines = entries.map(([k, v]) => `export ${k}=${v}`);
  appendFileSync(envFile, lines.join("\n") + "\n");
  log(`env vars: wrote ${entries.map(([k]) => k).join(", ")} to $CLAUDE_ENV_FILE.`);
}

// --- Step 2: project dependencies (only for the manifests/managers present) -
// mite is a monorepo with three independent dependency surfaces:
//   - the root Rust crate (Cargo.toml)            -> cargo fetch (lookup core)
//   - the Astro marketing site (site/package.json) -> npm install
//   - the Tauri app frontend (app/package.json)    -> bun install (CI uses bun)
// Each step fetches only; nothing is built (see the Windows-first note above).
// Steps self-gate on the manifest existing and the manager being on PATH, so the
// file is correct unedited in the cloud and on a fresh local clone.
function installDeps() {
  const steps = [
    {
      name: "root crate",
      manifest: ".",
      manager: "cargo",
      cmd: ["cargo", "fetch", "--locked"],
    },
    {
      name: "site",
      manifest: "site",
      manager: "npm",
      cmd: ["npm", "install"],
    },
    {
      name: "app frontend",
      manifest: "app",
      manager: "bun",
      cmd: ["bun", "install"],
    },
  ];

  for (const { name, manifest, manager, cmd } of steps) {
    const dir = join(repoRoot, manifest);
    const manifestFile =
      manager === "cargo" ? join(dir, "Cargo.toml") : join(dir, "package.json");
    if (!existsSync(manifestFile)) {
      log(`deps: ${name}: no manifest at ${manifest}; skipping.`);
      continue;
    }
    if (!has(manager)) {
      log(`deps: ${name}: ${manager} not on PATH; skipping (cloud-setup.sh installs it).`);
      continue;
    }
    try {
      // execSync's shell-string form invokes through the platform shell, which
      // resolves Windows .cmd shims (npm.cmd) and POSIX binaries alike. The
      // command is a static literal (no untrusted interpolation), so there is no
      // injection surface, and this avoids the DEP0190 warning that execFileSync
      // with `shell:true` emits.
      execSync(cmd.join(" "), { cwd: dir, stdio: "inherit" });
      log(`deps: ${name}: fetched via ${cmd.join(" ")}.`);
    } catch (err) {
      log(`deps: ${name}: ${cmd[0]} failed (${err.message}); continuing.`);
    }
  }
}

log(`starting (${isCloud ? "cloud" : "local"} session, root ${repoRoot}).`);
writeEnvVars();
installDeps();
log("done.");
