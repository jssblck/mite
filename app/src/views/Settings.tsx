import { useEffect, useRef, useState, type ReactNode } from "react";
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
  /** Called after a manual engine update or config reset, so the app can
   * re-warm the OCR engines against the new binary/config. */
  onEngineUpdated: () => void;
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
  const supportedTier: RuntimeTier = doctor.gpu_runtime?.tier ?? "cpu";
  const effectiveTier: RuntimeTier = doctor.gpu_runtime?.effective_tier ?? "cpu";
  // The NVIDIA install is capable but the engine's own ONNX Runtime provider
  // DLLs are missing, so it runs on the CPU. An engine update (above) fixes it.
  if (effectiveTier !== supportedTier) {
    return "GPU runtime ready, but the engine's ONNX Runtime libraries are missing; use Engine > Update.";
  }
  switch (effectiveTier) {
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
  onChooseEvalRoot,
  error,
  onClose,
}: {
  settings: AppSettings | null;
  onChange: (patch: Partial<AppSettings>) => void;
  onChooseEvalRoot: (enableAfterSelection: boolean) => void;
  error: string | null;
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
            label="Only while focused"
            detail="Hide the overlay and pause OCR whenever the watched window is not the active window."
          >
            <Toggle
              label="Only while focused"
              checked={settings?.watchFocusOnly ?? true}
              disabled={!settings}
              onChange={(v) => onChange({ watchFocusOnly: v })}
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

          <SettingRow
            label="Automatic eval capture"
            detail="Save a raw frame when the recognized text or layout changes enough to be a new scene."
          >
            <Toggle
              label="Automatic eval capture"
              checked={settings?.autoEvalCapture ?? false}
              disabled={!settings}
              onChange={(enabled) => {
                if (enabled && !settings?.evalCaptureRoot) {
                  onChooseEvalRoot(true);
                } else {
                  onChange({ autoEvalCapture: enabled });
                }
              }}
            />
          </SettingRow>

          {settings?.autoEvalCapture && (
            <SettingRow
              label="Eval capture root"
              detail="Each watched window gets a normalized subfolder here."
            >
              <div className="folder-control">
                <div className="folder-path" title={settings.evalCaptureRoot ?? ""}>
                  {settings.evalCaptureRoot ?? "No folder selected"}
                </div>
                <button
                  className="btn btn-ghost btn-sm"
                  onClick={() => onChooseEvalRoot(false)}
                >
                  Choose folder
                </button>
              </div>
            </SettingRow>
          )}
        </div>

        {error && (
          <div className="error-text" role="alert">
            {error}
          </div>
        )}

        <div className="btn-row modal-actions">
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

export function Settings({
  status,
  onRefresh,
  onOpenRuntimeSetup,
  onEngineUpdated,
}: SettingsProps) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const settingsRef = useRef<AppSettings | null>(null);
  const savedSettingsRef = useRef<AppSettings | null>(null);
  const watchSaveQueue = useRef<Promise<void>>(Promise.resolve());
  const [advancedOpen, setAdvancedOpen] = useState(false);

  useEffect(() => {
    api
      .getSettings()
      .then((loaded) => {
        savedSettingsRef.current = loaded;
        settingsRef.current = loaded;
        setSettings(loaded);
      })
      .catch(() => undefined);
  }, []);

  // Persist a watch-option change and reflect the saved settings the backend
  // returns, so the UI stays in lockstep with what the next launch will use.
  async function saveWatch(patch: Partial<AppSettings>) {
    const current = settingsRef.current;
    if (!current) return;
    const next = { ...current, ...patch };
    settingsRef.current = next;
    setSettings(next);
    setError(null);

    const operation = watchSaveQueue.current.then(async () => {
      try {
        const saved = await api.setWatchOptions(
          next.watchAuto,
          next.watchFocusOnly,
          next.watchHud,
          next.watchMetricsIntervalSecs,
          next.autoEvalCapture,
          next.evalCaptureRoot,
        );
        savedSettingsRef.current = saved;
        if (settingsRef.current === next) {
          settingsRef.current = saved;
          setSettings(saved);
          setError(null);
        }
      } catch (err) {
        const saved = savedSettingsRef.current;
        if (settingsRef.current === next && saved) {
          settingsRef.current = saved;
          setSettings(saved);
          setError(String(err));
        }
      }
    });
    watchSaveQueue.current = operation;
    await operation;
  }

  async function chooseEvalRoot(enableAfterSelection: boolean) {
    setError(null);
    try {
      const selected = await api.chooseEvalCaptureRoot();
      if (typeof selected !== "string") return;
      await saveWatch({
        evalCaptureRoot: selected,
        ...(enableAfterSelection ? { autoEvalCapture: true } : {}),
      });
    } catch (err) {
      setError(String(err));
    }
  }

  /** Run one settings action; resolves true only when it succeeded. */
  async function run(key: string, action: () => Promise<void>): Promise<boolean> {
    setBusy(key);
    setError(null);
    try {
      await action();
      onRefresh();
      return true;
    } catch (err) {
      setError(String(err));
      return false;
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
            onClick={() =>
              run("cli", () => api.installOrUpdateCli()).then((ok) => {
                if (ok) onEngineUpdated();
              })
            }
            disabled={busy !== null}
          >
            {busy === "cli" ? "Updating..." : "Update"}
          </button>
          <button
            className="btn btn-ghost btn-sm"
            onClick={() =>
              run("config", () => api.writeDefaultConfig()).then((ok) => {
                if (ok) onEngineUpdated();
              })
            }
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
          detail="Run mode, focus gating, latency HUD, and metrics logging while watching."
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
          onChooseEvalRoot={chooseEvalRoot}
          error={error}
          onClose={() => setAdvancedOpen(false)}
        />
      )}
    </div>
  );
}
