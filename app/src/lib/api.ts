// Typed wrappers over the Tauri command + event surface exposed by the Rust
// backend (see app/src-tauri/src/commands.rs). Argument names are camelCase on
// this side; Tauri maps them to the snake_case Rust parameters.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { check, type Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { open } from "@tauri-apps/plugin-dialog";

/** The detected runtime tier, slowest-safe to fastest. */
export type RuntimeTier = "cpu" | "cuda" | "tensor_rt";

/** Presence of one required NVIDIA runtime DLL. */
export interface DllPresence {
  name: string;
  present: boolean;
  found_in: string | null;
}

/** Presence of a whole tier's required DLL set. */
export interface TierStatus {
  present: boolean;
  components: DllPresence[];
}

/**
 * Presence of ONNX Runtime's provider bridge DLLs next to the engine. Unlike the
 * NVIDIA runtime, Mite ships these itself; when one is missing the engine cannot
 * register a GPU execution provider and runs on the CPU regardless of `tier`.
 */
export interface OrtProviderStatus {
  shared: DllPresence;
  cuda: DllPresence;
  tensorrt: DllPresence;
}

/**
 * The runtime detection from `doctor --json`. `tier` reflects only the user's
 * NVIDIA install (what the guided setup cares about); `effective_tier` is what
 * the engine can actually reach, i.e. `tier` gated by `ort_providers`. The
 * status surface should read `effective_tier`. Field names are snake_case
 * because this is the CLI's JSON verbatim.
 */
export interface GpuRuntimeStatus {
  tier: RuntimeTier;
  effective_tier: RuntimeTier;
  tensorrt: TierStatus;
  cuda: TierStatus;
  ort_providers: OrtProviderStatus;
  builder_present: boolean;
  nvrtc_present: boolean;
  dll_dirs: string[];
  searched_dirs: string[];
}

/** A subset of the CLI's `doctor --json` report that the UI surfaces. */
export interface DoctorReport {
  os: string;
  nvidia: {
    available: boolean;
    gpu_name: string | null;
    driver_version: string | null;
  };
  gpu_runtime: GpuRuntimeStatus | null;
  runtime_backend: string;
  warnings: string[];
  [key: string]: unknown;
}

export interface AppStatus {
  miteHome: string;
  appVersion: string;
  cliInstalled: boolean;
  cliVersion: string | null;
  modelsReady: boolean;
  runtimeSetupSeen: boolean;
  doctor: DoctorReport | null;
}

/** Persisted app settings: recorded runtime tier, DLL dirs, and watch options. */
export interface AppSettings {
  runtimeTier: RuntimeTier | null;
  dllDirs: string[];
  runtimeSetupSeen: boolean;
  watchAuto: boolean;
  watchFocusOnly: boolean;
  watchHud: boolean;
  watchMetricsIntervalSecs: number;
  autoEvalCapture: boolean;
  evalCaptureRoot: string | null;
}

/** How the installed engine relates to the engine this app build wants. */
export type EngineState = "ok" | "update" | "required" | "unknown";

export interface UpdateInfo {
  appVersion: string;
  /** The installed engine version, if the CLI is present. */
  currentCli: string | null;
  /** The engine version this app build should run (newest compatible). */
  targetCli: string | null;
  /** The release tag the target engine comes from. */
  targetTag: string | null;
  engineState: EngineState;
}

export interface WindowSummary {
  id: number;
  pid: number;
  title: string;
  appName: string;
  width: number;
  height: number;
  x: number;
  y: number;
  /**
   * WGC thumbnail as a PNG data URL, captured by the CLI alongside the listing.
   * Absent when the window could not be captured (the card shows a placeholder).
   */
  thumbnail?: string;
}

export interface DownloadProgress {
  task: string;
  file: string;
  received: number;
  total: number;
  done: boolean;
}

export interface WatchLog {
  line: string;
  stream: "stdout" | "stderr";
}

export interface WatchStateEvent {
  running: boolean;
  code: number | null;
}

/**
 * One progress event from `mite warmup --json`, forwarded verbatim by the
 * backend supervisor. `target` names a model session ("detector",
 * "recognizer", "fallback_recognizer"); `provider` is the execution provider
 * it landed on ("tensor_rt", "cuda", "cpu").
 */
export type WarmupEvent =
  | { event: "start"; backend: string; targets: string[] }
  | { event: "build_started"; target: string; likelyCompile: boolean }
  | {
      event: "build_finished";
      target: string;
      provider: string;
      elapsedMs: number;
    }
  | { event: "warm_started"; target: string }
  | { event: "warm_finished"; target: string; elapsedMs: number }
  | { event: "done"; elapsedMs: number; providers: Record<string, string> };

/** Warmup child lifecycle: start, and exit with a stderr tail on failure. */
export interface WarmupStateEvent {
  running: boolean;
  code: number | null;
  error: string | null;
}

export const api = {
  appVersion: () => invoke<string>("app_version"),
  miteHomePath: () => invoke<string>("mite_home_path"),
  getStatus: () => invoke<AppStatus>("get_status"),
  checkForUpdates: () => invoke<UpdateInfo>("check_for_updates"),
  installOrUpdateCli: () => invoke<void>("install_or_update_cli"),
  downloadModels: () => invoke<void>("download_models"),
  detectRuntime: () => invoke<DoctorReport>("detect_runtime"),
  recordRuntime: () => invoke<AppSettings>("record_runtime"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  setWatchOptions: (
    auto: boolean,
    focusOnly: boolean,
    hud: boolean,
    metricsIntervalSecs: number,
    autoEvalCapture: boolean,
    evalCaptureRoot: string | null,
  ) =>
    invoke<AppSettings>("set_watch_options", {
      auto,
      focusOnly,
      hud,
      metricsIntervalSecs,
      autoEvalCapture,
      evalCaptureRoot,
    }),
  chooseEvalCaptureRoot: () =>
    open({
      directory: true,
      multiple: false,
      title: "Choose eval capture root",
    }),
  pipAvailable: () => invoke<boolean>("pip_available"),
  writeDefaultConfig: () => invoke<void>("write_default_config"),
  listWindows: () => invoke<WindowSummary[]>("list_windows"),
  startWatch: (windowId: number, windowTitle: string) =>
    invoke<void>("start_watch", { windowId, windowTitle }),
  stopWatch: () => invoke<void>("stop_watch"),
  isWatching: () => invoke<boolean>("is_watching"),
  startWarmup: () => invoke<void>("start_warmup"),
  isWarming: () => invoke<boolean>("is_warming"),
  openMiteHome: () => invoke<void>("open_mite_home"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  uninstallData: () => invoke<void>("uninstall_data"),
};

export function onDownloadProgress(
  cb: (progress: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>("download-progress", (event) => cb(event.payload));
}

export function onWatchLog(cb: (log: WatchLog) => void): Promise<UnlistenFn> {
  return listen<WatchLog>("watch-log", (event) => cb(event.payload));
}

export function onWatchState(
  cb: (state: WatchStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<WatchStateEvent>("watch-state", (event) => cb(event.payload));
}

export function onWarmupEvent(
  cb: (event: WarmupEvent) => void,
): Promise<UnlistenFn> {
  return listen<WarmupEvent>("warmup-event", (event) => cb(event.payload));
}

export function onWarmupState(
  cb: (state: WarmupStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<WarmupStateEvent>("warmup-state", (event) => cb(event.payload));
}

/** Handle to a pending signed app update returned by `appUpdater.check()`. */
export type AppUpdate = Update;
/** Download lifecycle event passed to `update.downloadAndInstall(cb)`. */
export type AppUpdateEvent = DownloadEvent;

/**
 * Signed self-update for the desktop app itself (distinct from the mite CLI
 * update path above). Backed by tauri-plugin-updater, which verifies each
 * release's installer against the minisign public key baked into the build.
 * These calls only resolve inside the packaged app; in `tauri dev` there is no
 * updater runtime and `check()` rejects.
 */
export const appUpdater = {
  /** A handle when a newer signed release is published, otherwise null. */
  check: (): Promise<AppUpdate | null> => check(),
  /** Relaunch into the freshly installed version. */
  relaunch: (): Promise<void> => relaunch(),
};
