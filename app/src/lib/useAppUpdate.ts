import { useCallback, useEffect, useRef, useState } from "react";
import { appUpdater, type AppUpdate } from "./api";

export type AppUpdatePhase =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "uptodate"
  | "error";

export interface AppUpdateState {
  phase: AppUpdatePhase;
  update: AppUpdate | null;
  error: string | null;
  received: number;
  total: number;
  /** True while a newer signed build is available, downloading, or staged. */
  pending: boolean;
  /** Manually re-check the release feed (surfaces errors). */
  check: () => Promise<void>;
  /** Download, verify, and stage the available update. */
  install: () => Promise<void>;
  /** Relaunch into the freshly staged version. */
  restart: () => Promise<void>;
}

/**
 * Lifecycle for the desktop app's signed self-update, shared by the priority
 * banner on the dashboard and the manual control in Settings.
 *
 * When `autoCheck` is true the feed is polled once on mount and failures are
 * swallowed (there is no updater runtime under `tauri dev`, so `check()` rejects
 * there): the banner simply does not appear. A manual `check()` surfaces errors.
 */
export function useAppUpdate(autoCheck = false): AppUpdateState {
  const [phase, setPhase] = useState<AppUpdatePhase>("idle");
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [received, setReceived] = useState(0);
  const [total, setTotal] = useState(0);
  const autoChecked = useRef(false);

  const check = useCallback(async () => {
    setPhase("checking");
    setError(null);
    try {
      const found = await appUpdater.check();
      if (found) {
        setUpdate(found);
        setPhase("available");
      } else {
        setPhase("uptodate");
      }
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }, []);

  const install = useCallback(async () => {
    if (!update) return;
    setPhase("downloading");
    setError(null);
    setReceived(0);
    setTotal(0);
    let got = 0;
    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            got = 0;
            setReceived(0);
            setTotal(event.data.contentLength ?? 0);
            break;
          case "Progress":
            got += event.data.chunkLength;
            setReceived(got);
            break;
          case "Finished":
            break;
        }
      });
      setPhase("ready");
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }, [update]);

  const restart = useCallback(async () => {
    setError(null);
    try {
      await appUpdater.relaunch();
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }, []);

  useEffect(() => {
    if (!autoCheck || autoChecked.current) return;
    autoChecked.current = true;
    appUpdater
      .check()
      .then((found) => {
        if (found) {
          setUpdate(found);
          setPhase("available");
        } else {
          setPhase("uptodate");
        }
      })
      .catch(() => setPhase("idle"));
  }, [autoCheck]);

  return {
    phase,
    update,
    error,
    received,
    total,
    pending: phase === "available" || phase === "downloading" || phase === "ready",
    check,
    install,
    restart,
  };
}
