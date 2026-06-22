import { useState, type ReactNode } from "react";
import { api, type AppStatus, type RuntimeTier } from "../lib/api";
import { AppUpdateRow } from "../components/AppUpdateRow";

interface SettingsProps {
  status: AppStatus;
  onRefresh: () => void;
  onOpenRuntimeSetup: () => void;
}

function runtimeSummary(status: AppStatus): string {
  const doctor = status.doctor;
  if (!doctor) return "Not yet detected.";
  if (!doctor.nvidia.available) return "No NVIDIA GPU; running on the CPU.";
  const tier: RuntimeTier = doctor.gpu_runtime?.tier ?? "cpu";
  switch (tier) {
    case "tensor_rt":
      return "TensorRT active (fastest path).";
    case "cuda":
      return "CUDA active (about 2x slower than TensorRT).";
    default:
      return "Running on the CPU.";
  }
}

/** One settings row: label and short detail on the left, controls on the right. */
export function SettingRow({
  label,
  detail,
  children,
}: {
  label: string;
  detail: string;
  children: ReactNode;
}) {
  return (
    <div className="setting-row">
      <div className="setting-main">
        <div className="setting-label">{label}</div>
        <div className="setting-detail">{detail}</div>
      </div>
      <div className="setting-actions">{children}</div>
    </div>
  );
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
      </div>

      <div className="card settings-list">
        <SettingRow
          label="Engine"
          detail={
            status.cliVersion ? `Installed ${status.cliVersion}` : "Not installed"
          }
        >
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => run("cli", () => api.installOrUpdateCli())}
            disabled={busy !== null}
          >
            {busy === "cli" ? "Updating..." : "Update"}
          </button>
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => run("config", () => api.writeDefaultConfig())}
            disabled={busy !== null}
          >
            {busy === "config" ? "Resetting..." : "Reset config"}
          </button>
        </SettingRow>

        <AppUpdateRow appVersion={status.appVersion} />

        <SettingRow label="GPU acceleration" detail={runtimeSummary(status)}>
          <button
            className="btn btn-ghost btn-sm"
            onClick={onOpenRuntimeSetup}
            disabled={busy !== null}
          >
            {tier === "tensor_rt" ? "Re-run setup" : "Set up"}
          </button>
        </SettingRow>

        <SettingRow label="Storage" detail="Engine, models, cache, and config.">
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => api.openMiteHome().catch((e) => setError(String(e)))}
          >
            Open folder
          </button>
        </SettingRow>

        <SettingRow
          label="Remove data"
          detail="Deletes models, cache, and config; re-downloadable anytime."
        >
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
                {busy === "wipe" ? "Removing..." : "Confirm"}
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
              Remove
            </button>
          )}
        </SettingRow>

        {error && <div className="error-text">{error}</div>}
      </div>
    </div>
  );
}
