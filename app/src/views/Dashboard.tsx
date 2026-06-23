import { useEffect } from "react";
import { type AppStatus } from "../lib/api";

interface DashboardProps {
  status: AppStatus;
  watching: boolean;
  onRefresh: () => void;
  onWatch: () => void;
  onSetupGpu: () => void;
}

/** The leading sentence of a diagnostics message, for a compact one-line view. */
function firstSentence(text: string): string {
  const end = text.indexOf(". ");
  return end === -1 ? text : text.slice(0, end + 1);
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

export function Dashboard({
  status,
  watching,
  onRefresh,
  onWatch,
  onSetupGpu,
}: DashboardProps) {
  // Auto-refresh the install status while the Dashboard is on screen (it only
  // mounts when its tab is active). Debounced so a slow probe never overlaps the
  // next tick, paused while the window is hidden, and re-run on regaining focus.
  useEffect(() => {
    let inFlight = false;
    const tick = async () => {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        await onRefresh();
      } finally {
        inFlight = false;
      }
    };
    tick();
    const handle = setInterval(tick, 3000);
    const onVisible = () => {
      if (!document.hidden) tick();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(handle);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [onRefresh]);

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

  // An NVIDIA GPU is present but not at the fastest path: setup can still help.
  const needsGpuSetup = Boolean(gpu?.available) && tier !== "tensor_rt";

  return (
    <div>
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
            detail={firstSentence(doctor.warnings[0])}
          />
        )}
        <div className="btn-row">
          <button className="btn btn-primary" onClick={onWatch}>
            {watching ? "Go to watch" : "Start watching"}
          </button>
          {needsGpuSetup && (
            <button className="btn btn-ghost" onClick={onSetupGpu}>
              Set up GPU
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
