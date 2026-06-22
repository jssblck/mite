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

type Phase = "intro" | "running";

export function SetupWizard({ status, onDone }: SetupWizardProps) {
  const [phase, setPhase] = useState<Phase>("intro");
  const [statuses, setStatuses] = useState<Record<StepId, StepStatus>>({
    cli: status.cliInstalled ? "done" : "pending",
    config: "pending",
    models: status.modelsReady ? "done" : "pending",
  });
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
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
      // GPU acceleration is a separate, guided step handled after the core
      // install completes (the app opens it when an NVIDIA GPU is present).
      onDone();
    } catch (err) {
      const active = (Object.keys(statuses) as StepId[]).find(
        (id) => statuses[id] === "active",
      );
      if (active) setStep(active, "error");
      setError(String(err));
      setPhase("running");
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
        </div>
      </main>
    </div>
  );
}
