import { useEffect, useRef, useState } from "react";
import {
  api,
  onDownloadProgress,
  type AppStatus,
  type DownloadProgress,
} from "../lib/api";
import { MiteMark } from "../components/MiteMark";
import { ProgressBar } from "../components/ProgressBar";

interface SetupWizardProps {
  status: AppStatus;
  onDone: () => void;
}

type StepId = "cli" | "config" | "models";
type StepStatus = "pending" | "active" | "done" | "error";

interface StepView {
  id: StepId;
  title: string;
  detail: string;
}

const STEPS: StepView[] = [
  { id: "cli", title: "Install the mite engine", detail: "Downloads the latest mite CLI." },
  { id: "config", title: "Write configuration", detail: "Creates a default mite.toml." },
  { id: "models", title: "Download recognition models", detail: "OCR models and the JMdict dictionary (a few hundred MB)." },
];

type Phase = "intro" | "running" | "gpu" | "done";

export function SetupWizard({ status, onDone }: SetupWizardProps) {
  const [phase, setPhase] = useState<Phase>("intro");
  const [statuses, setStatuses] = useState<Record<StepId, StepStatus>>({
    cli: status.cliInstalled ? "done" : "pending",
    config: "pending",
    models: status.modelsReady ? "done" : "pending",
  });
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [gpuState, setGpuState] = useState<StepStatus>(
    status.gpuPackInstalled ? "done" : "pending",
  );
  const progressRef = useRef<DownloadProgress | null>(null);

  useEffect(() => {
    const unlisten = onDownloadProgress((p) => {
      progressRef.current = p;
      setProgress(p);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  function setStep(id: StepId, value: StepStatus) {
    setStatuses((prev) => ({ ...prev, [id]: value }));
  }

  async function runStep(id: StepId, action: () => Promise<void>) {
    if (statuses[id] === "done") return;
    setStep(id, "active");
    setProgress(null);
    await action();
    setStep(id, "done");
  }

  async function runAll() {
    setError(null);
    setPhase("running");
    try {
      await runStep("cli", () => api.installOrUpdateCli());
      await runStep("config", () => api.writeDefaultConfig());
      await runStep("models", () => api.downloadModels());
      setPhase("gpu");
    } catch (err) {
      const active = (Object.keys(statuses) as StepId[]).find(
        (id) => statuses[id] === "active",
      );
      if (active) setStep(active, "error");
      setError(String(err));
      setPhase("running");
    }
  }

  async function installGpu() {
    setError(null);
    setGpuState("active");
    setProgress(null);
    try {
      await api.downloadGpuPack();
      setGpuState("done");
    } catch (err) {
      setGpuState("error");
      setError(String(err));
    }
  }

  const activeProgressTask =
    progress && !progress.done ? progress : null;

  return (
    <div className="app-shell">
      <main className="app-main">
        <div className="wizard">
          <div className="wizard-hero">
            <MiteMark className="mark" size="2.75rem" />
            <h1>Set up Mite</h1>
            <p>
              Mite reads Japanese text on screen and defines it on hover. This
              one-time setup downloads the engine and its recognition models.
            </p>
          </div>

          <div className="card">
            {STEPS.map((step, index) => {
              const state = statuses[step.id];
              return (
                <div
                  key={step.id}
                  className={`step${state === "active" ? " active" : ""}${state === "done" ? " done" : ""}`}
                >
                  <div className="step-index">
                    {state === "done" ? "✓" : index + 1}
                  </div>
                  <div className="step-body">
                    <div className="step-title">{step.title}</div>
                    <div className={`step-detail${state === "error" ? " error" : ""}`}>
                      {state === "active" && activeProgressTask
                        ? `Downloading ${activeProgressTask.file}...`
                        : step.detail}
                    </div>
                    {state === "active" && activeProgressTask && (
                      <div style={{ marginTop: "0.5rem" }}>
                        <ProgressBar
                          received={activeProgressTask.received}
                          total={activeProgressTask.total}
                        />
                      </div>
                    )}
                  </div>
                </div>
              );
            })}

            {error && <div className="error-text">{error}</div>}

            <div className="btn-row">
              {phase === "intro" && (
                <button className="btn btn-primary" onClick={runAll}>
                  Begin setup
                </button>
              )}
              {phase === "running" && error && (
                <button className="btn btn-primary" onClick={runAll}>
                  Retry
                </button>
              )}
              {phase === "running" && !error && (
                <button className="btn btn-ghost" disabled>
                  <span className="inline-spinner" /> Working...
                </button>
              )}
            </div>
          </div>

          {phase === "gpu" && (
            <div className="card">
              <div className="card-title">
                <MiteMark size="1.1rem" /> Optional: GPU acceleration
              </div>
              <p className="card-sub">
                If you have an NVIDIA GPU, the acceleration pack makes recognition
                dramatically faster. It is a large download (several GB) and can
                be added later from Settings. Mite runs without it on the CPU.
              </p>
              {gpuState === "active" && activeProgressTask && (
                <ProgressBar
                  received={activeProgressTask.received}
                  total={activeProgressTask.total}
                  label="GPU runtime"
                />
              )}
              {error && <div className="error-text">{error}</div>}
              <div className="btn-row">
                <button
                  className="btn btn-primary"
                  onClick={installGpu}
                  disabled={gpuState === "active" || gpuState === "done"}
                >
                  {gpuState === "done"
                    ? "Installed"
                    : gpuState === "active"
                      ? "Downloading..."
                      : "Install GPU pack"}
                </button>
                <button className="btn btn-ghost" onClick={onDone}>
                  {gpuState === "done" ? "Finish" : "Skip for now"}
                </button>
              </div>
            </div>
          )}
        </div>
      </main>
    </div>
  );
}
