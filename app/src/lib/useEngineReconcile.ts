import { useEffect, useRef, useState } from "react";
import { api, onDownloadProgress } from "./api";

export type EngineReconcilePhase =
  | "idle"
  | "checking"
  | "downloading"
  | "done"
  | "error";

export interface EngineReconcileState {
  phase: EngineReconcilePhase;
  /** The engine version being installed, when known. */
  version: string | null;
  error: string | null;
  received: number;
  total: number;
}

/**
 * Keep the installed engine in lockstep with the running app build, silently.
 *
 * On the first launch where the app is ready, this resolves the engine version
 * this app build wants (the newest within its compatible semver range) and, if
 * the installed engine is older or out of range, downloads it with no prompt.
 * The app shell updates itself through a prompted banner; the engine it pulls is
 * a consequence of which app version is running, so it needs no confirmation.
 *
 * Runs once per launch. After a prompted app self-update relaunches the process,
 * the next launch reconciles the engine to the new app's range automatically.
 */
export function useEngineReconcile(
  enabled: boolean,
  onRefresh: () => void,
): EngineReconcileState {
  const [phase, setPhase] = useState<EngineReconcilePhase>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [received, setReceived] = useState(0);
  const [total, setTotal] = useState(0);
  const ran = useRef(false);

  useEffect(() => {
    if (!enabled || ran.current) return;
    ran.current = true;
    let cancelled = false;
    let unlisten: ReturnType<typeof onDownloadProgress> | null = null;

    (async () => {
      setPhase("checking");
      try {
        const info = await api.checkForUpdates();
        if (cancelled) return;
        if (info.engineState !== "update" && info.engineState !== "required") {
          setPhase("idle");
          return;
        }
        setVersion(info.targetCli);
        setReceived(0);
        setTotal(0);
        setPhase("downloading");
        unlisten = onDownloadProgress((p) => {
          if (p.task !== "cli") return;
          setReceived(p.received);
          setTotal(p.total);
        });
        await api.installOrUpdateCli();
        if (cancelled) return;
        setPhase("done");
        onRefresh();
      } catch (err) {
        if (!cancelled) {
          setError(String(err));
          setPhase("error");
        }
      } finally {
        unlisten?.then((fn) => fn());
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.then((fn) => fn());
    };
  }, [enabled, onRefresh]);

  return { phase, version, error, received, total };
}
