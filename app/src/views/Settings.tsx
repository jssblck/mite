import { useEffect, useState, type ReactNode } from "react";
import {
  api,
  type AppSettings,
  type AppStatus,
  type RuntimeTier,
} from "../lib/api";
import { AppUpdateRow } from "../components/AppUpdateRow";

interface SettingsProps {
  status: AppStatus;
  onRefresh: () => void;
  onOpenRuntimeSetup: () => void;
}

/** A native-style on/off toggle backed by a checkbox. */
function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <label className="switch" aria-label={label}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="switch-track" aria-hidden="true">
        <span className="switch-thumb" />
      </span>
    </label>
  );
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

/** The "Advanced options" modal: how the overlay behaves while watching. */
function AdvancedOptionsModal({
  settings,
  onChange,
  onClose,
}: {
  settings: AppSettings | null;
  onChange: (patch: Partial<AppSettings>) => void;
  onClose: () => void;
}) {
  return (
    <div
      className="modal-overlay"
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="modal modal-wide"
        role="dialog"
        aria-modal="true"
        aria-label="Advanced options"
      >
        <div className="modal-head">
          <h2 className="modal-title">Advanced options</h2>
          <button className="icon-btn" aria-label="Close" onClick={onClose}>
            <svg
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </div>

        <div className="settings-list">
          <SettingRow
            label="Run continuously"
            detail="Keep the overlay active instead of holding Shift (some games swallow Shift)."
          >
            <Toggle
              label="Run continuously"
              checked={settings?.watchAuto ?? true}
              disabled={!settings}
              onChange={(v) => onChange({ watchAuto: v })}
            />
          </SettingRow>

          <SettingRow
            label="Latency HUD"
            detail="Show per-stage timings overlaid while watching."
          >
            <Toggle
              label="Latency HUD"
              checked={settings?.watchHud ?? false}
              disabled={!settings}
              onChange={(v) => onChange({ watchHud: v })}
            />
          </SettingRow>

          <SettingRow
            label="Metrics logging"
            detail="Log aggregate latency to the engine output every N seconds (0 = off)."
          >
            <input
              className="number-input"
              type="number"
              min={0}
              aria-label="Metrics interval in seconds"
              value={settings?.watchMetricsIntervalSecs ?? 0}
              disabled={!settings}
              onChange={(e) =>
                onChange({
                  watchMetricsIntervalSecs: Math.max(
                    0,
                    Number(e.target.value) || 0,
                  ),
                })
              }
            />
          </SettingRow>
        </div>

        <div className="btn-row modal-actions">
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

export function Settings({ status, onRefresh, onOpenRuntimeSetup }: SettingsProps) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => undefined);
  }, []);

  // Persist a watch-option change and reflect the saved settings the backend
  // returns, so the UI stays in lockstep with what the next launch will use.
  async function saveWatch(patch: Partial<AppSettings>) {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    try {
      const saved = await api.setWatchOptions(
        next.watchAuto,
        next.watchHud,
        next.watchMetricsIntervalSecs,
      );
      setSettings(saved);
    } catch (err) {
      setError(String(err));
    }
  }

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

        <SettingRow
          label="Watching"
          detail="Run mode, latency HUD, and metrics logging while watching."
        >
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => setAdvancedOpen(true)}
          >
            Advanced options
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

      {advancedOpen && (
        <AdvancedOptionsModal
          settings={settings}
          onChange={saveWatch}
          onClose={() => setAdvancedOpen(false)}
        />
      )}
    </div>
  );
}
