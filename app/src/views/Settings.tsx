import { useEffect, useState } from "react";
import {
  api,
  onDownloadProgress,
  type AppStatus,
  type DownloadProgress,
} from "../lib/api";
import { ProgressBar } from "../components/ProgressBar";
import { AppUpdateCard } from "../components/AppUpdateCard";

interface SettingsProps {
  status: AppStatus;
  onRefresh: () => void;
}

export function Settings({ status, onRefresh }: SettingsProps) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);

  useEffect(() => {
    const unlisten = onDownloadProgress(setProgress);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  async function run(key: string, action: () => Promise<void>) {
    setBusy(key);
    setError(null);
    setProgress(null);
    try {
      await action();
      onRefresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  const downloading =
    busy === "gpu" && progress && !progress.done ? progress : null;

  return (
    <div>
      <div className="page-head">
        <h1>Settings</h1>
        <p>Manage the engine, its dependencies, and where Mite stores files.</p>
      </div>

      <div className="card">
        <div className="card-title">Engine</div>
        <p className="card-sub">
          Installed version: {status.cliVersion ?? "not installed"}. App version:{" "}
          {status.appVersion}.
        </p>
        <div className="btn-row">
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => run("cli", () => api.installOrUpdateCli())}
            disabled={busy !== null}
          >
            {busy === "cli" ? "Updating..." : "Update engine"}
          </button>
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => run("config", () => api.writeDefaultConfig())}
            disabled={busy !== null}
          >
            {busy === "config" ? "Writing..." : "Reset configuration"}
          </button>
        </div>
      </div>

      <AppUpdateCard appVersion={status.appVersion} />

      <div className="card">
        <div className="card-title">GPU acceleration</div>
        <p className="card-sub">
          {status.gpuPackInstalled
            ? "The GPU acceleration pack is installed."
            : "Not installed. Mite is running on the CPU. The pack is a large download (several GB) and needs an NVIDIA GPU."}
        </p>
        {downloading && (
          <ProgressBar
            received={downloading.received}
            total={downloading.total}
            label="GPU runtime"
          />
        )}
        <div className="btn-row">
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => run("gpu", () => api.downloadGpuPack())}
            disabled={busy !== null}
          >
            {busy === "gpu"
              ? "Downloading..."
              : status.gpuPackInstalled
                ? "Reinstall GPU pack"
                : "Install GPU pack"}
          </button>
        </div>
      </div>

      <div className="card">
        <div className="card-title">Storage</div>
        <p className="card-sub">
          Mite keeps the engine, models, and cache here:
        </p>
        <div className="code-path">{status.miteHome}</div>
        <div className="btn-row">
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => api.openMiteHome().catch((e) => setError(String(e)))}
          >
            Open folder
          </button>
          <button className="btn btn-ghost btn-sm" onClick={onRefresh}>
            Re-run diagnostics
          </button>
        </div>
      </div>

      <div className="card">
        <div className="card-title">Remove data</div>
        <p className="card-sub">
          Deletes downloaded models, the GPU pack, the engine cache, and config.
          The app and engine binary stay installed; you can re-download anytime.
        </p>
        {error && <div className="error-text">{error}</div>}
        <div className="btn-row">
          {confirmWipe ? (
            <>
              <button
                className="btn btn-danger btn-sm"
                onClick={() =>
                  run("wipe", () => api.uninstallData()).then(() =>
                    setConfirmWipe(false),
                  )
                }
                disabled={busy !== null}
              >
                {busy === "wipe" ? "Removing..." : "Confirm: remove data"}
              </button>
              <button
                className="btn btn-ghost btn-sm"
                onClick={() => setConfirmWipe(false)}
              >
                Cancel
              </button>
            </>
          ) : (
            <button
              className="btn btn-danger btn-sm"
              onClick={() => setConfirmWipe(true)}
            >
              Remove downloaded data
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
