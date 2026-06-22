// Typed wrappers over the Tauri command + event surface exposed by the Rust
// backend (see app/src-tauri/src/commands.rs). Argument names are camelCase on
// this side; Tauri maps them to the snake_case Rust parameters.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A subset of the CLI's `doctor --json` report that the UI surfaces. */
export interface DoctorReport {
  os: string;
  nvidia: {
    available: boolean;
    gpu_name: string | null;
    driver_version: string | null;
  };
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
  gpuPackInstalled: boolean;
  doctor: DoctorReport | null;
}

export interface UpdateInfo {
  currentCli: string | null;
  latestTag: string | null;
  latestCli: string | null;
  cliUpdateAvailable: boolean;
  appVersion: string;
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

export interface WatchOptions {
  windowId: number;
  auto: boolean;
  hud: boolean;
  metricsIntervalSecs: number;
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
  downloadGpuPack: () => invoke<void>("download_gpu_pack"),
  writeDefaultConfig: () => invoke<void>("write_default_config"),
  listWindows: () => invoke<WindowSummary[]>("list_windows"),
  captureThumbnail: (windowId: number, maxWidth: number) =>
    invoke<string>("capture_thumbnail", { windowId, maxWidth }),
  startWatch: (options: WatchOptions) => invoke<void>("start_watch", { options }),
  stopWatch: () => invoke<void>("stop_watch"),
  isWatching: () => invoke<boolean>("is_watching"),
  openMiteHome: () => invoke<void>("open_mite_home"),
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
