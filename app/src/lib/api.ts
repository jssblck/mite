// Typed wrappers over the Tauri command + event surface exposed by the Rust
// backend (see app/src-tauri/src/commands.rs). Argument names are camelCase on
// this side; Tauri maps them to the snake_case Rust parameters.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { check, type Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

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
 * The system-wide NVIDIA runtime detection. Mite never installs these binaries;
 * this only reports what the user has installed from NVIDIA and which tier it
 * supports. Field names are snake_case because this is the CLI's JSON verbatim.
 */
export interface GpuRuntimeStatus {
  tier: RuntimeTier;
  tensorrt: TierStatus;
  cuda: TierStatus;
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
  watchHud: boolean;
  watchMetricsIntervalSecs: number;
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
  setWatchOptions: (auto: boolean, hud: boolean, metricsIntervalSecs: number) =>
    invoke<AppSettings>("set_watch_options", { auto, hud, metricsIntervalSecs }),
  pipAvailable: () => invoke<boolean>("pip_available"),
  writeDefaultConfig: () => invoke<void>("write_default_config"),
  listWindows: () => invoke<WindowSummary[]>("list_windows"),
  captureThumbnail: (windowId: number, maxWidth: number) =>
    invoke<string>("capture_thumbnail", { windowId, maxWidth }),
  startWatch: (windowId: number) => invoke<void>("start_watch", { windowId }),
  stopWatch: () => invoke<void>("stop_watch"),
  isWatching: () => invoke<boolean>("is_watching"),
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
