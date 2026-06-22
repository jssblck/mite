import { useEffect, useState } from "react";
import { api, type AppStatus, type UpdateInfo } from "../lib/api";

interface DashboardProps {
  status: AppStatus;
  watching: boolean;
  onRefresh: () => void;
  onWatch: () => void;
}

function Stat({
  ok,
  label,
  detail,
}: {
  ok: "ok" | "warn" | "bad";
  label: string;
  detail: string;
}) {
  const glyph = ok === "ok" ? "✓" : ok === "warn" ? "!" : "×";
  return (
    <div className="stat">
      <div className={`stat-icon ${ok}`}>{glyph}</div>
      <div className="stat-body">
        <div className="stat-label">{label}</div>
        <div className="stat-detail">{detail}</div>
      </div>
    </div>
  );
}

export function Dashboard({ status, watching, onRefresh, onWatch }: DashboardProps) {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [updating, setUpdating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.checkForUpdates().then(setUpdate).catch(() => setUpdate(null));
  }, []);

  const doctor = status.doctor;
  const gpu = doctor?.nvidia;
  const tier = doctor?.gpu_runtime?.tier ?? "cpu";
  const accel =
    tier === "tensor_rt"
      ? { ok: "ok" as const, detail: "TensorRT (fastest path)" }
      : tier === "cuda"
        ? { ok: "warn" as const, detail: "CUDA backend (TensorRT not installed)" }
        : gpu?.available
          ? {
              ok: "warn" as const,
              detail: "Running on CPU. Set up GPU acceleration from Settings.",
            }
          : { ok: "warn" as const, detail: "Running on CPU (no NVIDIA GPU)." };

  async function updateCli() {
    setUpdating(true);
    setError(null);
    try {
      await api.installOrUpdateCli();
      const fresh = await api.checkForUpdates();
      setUpdate(fresh);
      onRefresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setUpdating(false);
    }
  }

  return (
    <div>
      <div className="page-head">
        <h1>Dashboard</h1>
        <p>Your Mite install at a glance, and one button to start reading.</p>
      </div>

      {update?.cliUpdateAvailable && (
        <div className="banner">
          <span className="banner-text">
            A newer mite engine is available
            {update.latestCli ? ` (${update.latestCli})` : ""}.
          </span>
          <button
            className="btn btn-sm btn-primary"
            onClick={updateCli}
            disabled={updating}
          >
            {updating ? "Updating..." : "Update"}
          </button>
        </div>
      )}

      <div className="card">
        <div className="card-title">Status</div>
        <Stat
          ok={status.cliInstalled ? "ok" : "bad"}
          label="Mite engine"
          detail={
            status.cliVersion ? `Installed (${status.cliVersion})` : "Not installed"
          }
        />
        <Stat
          ok={status.modelsReady ? "ok" : "bad"}
          label="Recognition models"
          detail={status.modelsReady ? "Ready" : "Missing"}
        />
        <Stat ok={accel.ok} label="GPU acceleration" detail={accel.detail} />
        {gpu && (
          <Stat
            ok={gpu.available ? "ok" : "warn"}
            label="Graphics card"
            detail={
              gpu.available
                ? `${gpu.gpu_name ?? "NVIDIA GPU"}${gpu.driver_version ? `, driver ${gpu.driver_version}` : ""}`
                : "No NVIDIA GPU detected"
            }
          />
        )}
        {doctor && doctor.warnings.length > 0 && (
          <Stat
            ok="warn"
            label="Diagnostics"
            detail={doctor.warnings.join(" ")}
          />
        )}
        {error && <div className="error-text">{error}</div>}
        <div className="btn-row">
          <button className="btn btn-primary" onClick={onWatch}>
            {watching ? "Go to watch" : "Start watching"}
          </button>
          <button className="btn btn-ghost btn-sm" onClick={onRefresh}>
            Refresh
          </button>
        </div>
      </div>
    </div>
  );
}
