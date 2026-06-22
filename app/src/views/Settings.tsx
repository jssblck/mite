import { useState } from "react";
import { api, type AppStatus, type RuntimeTier } from "../lib/api";
import { AppUpdateCard } from "../components/AppUpdateCard";

interface SettingsProps {
  status: AppStatus;
  onRefresh: () => void;
  onOpenRuntimeSetup: () => void;
}

function runtimeSummary(status: AppStatus): string {
  const doctor = status.doctor;
  if (!doctor) return "Run diagnostics to detect your GPU runtime.";
  if (!doctor.nvidia.available) {
    return "No NVIDIA GPU detected. Mite runs on the CPU.";
  }
  const tier: RuntimeTier = doctor.gpu_runtime?.tier ?? "cpu";
  switch (tier) {
    case "tensor_rt":
      return "TensorRT runtime detected. Mite uses the fastest path.";
    case "cuda":
      return "CUDA runtime detected. Mite uses the CUDA backend (roughly 2x slower than TensorRT). Install TensorRT for the fastest path.";
    default:
      return "An NVIDIA GPU is present but no GPU runtime is installed, so Mite runs on the CPU.";
  }
}

export function Settings({ status, onRefresh, onOpenRuntimeSetup }: SettingsProps) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);

  async function run(key: string, action: () => Promise<void>) {
    setBusy(key);
    setError(null);
    try {
      await action();
      onRefresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  const tier: RuntimeTier = status.doctor?.gpu_runtime?.tier ?? "cpu";

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
        <p className="card-sub">{runtimeSummary(status)}</p>
        <p className="card-sub subtle">
          NVIDIA's license does not let us install their runtime for you, so you
          install it yourself from NVIDIA. This guided step detects what is
          present and walks you through the rest.
        </p>
        <div className="btn-row">
          <button
            className="btn btn-ghost btn-sm"
            onClick={onOpenRuntimeSetup}
            disabled={busy !== null}
          >
            {tier === "tensor_rt" ? "Re-run GPU setup" : "Set up GPU acceleration"}
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
          Deletes downloaded models, the engine cache, and config. The app and
          engine binary stay installed, and your NVIDIA runtime is left
          untouched; you can re-download anytime.
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
