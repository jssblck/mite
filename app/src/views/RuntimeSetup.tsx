import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  type AppStatus,
  type DoctorReport,
  type GpuRuntimeStatus,
  type RuntimeTier,
} from "../lib/api";
import { MiteMark } from "../components/MiteMark";

interface RuntimeSetupProps {
  status: AppStatus;
  onClose: () => void;
}

/** NVIDIA's own package index, where the TensorRT runtime wheel is hosted. */
const NVIDIA_PIP_INDEX = "https://pypi.nvidia.com";

/**
 * The pinned NVIDIA packages, surfaced in the pip route's copy-paste command.
 * Mite loads the native DLLs through ONNX Runtime, so this installs the TensorRT
 * runtime libraries (`tensorrt-cu12-libs`) rather than the `tensorrt-cu12`
 * meta-package: that meta-package also pulls the Python bindings, which have no
 * wheel for current Python versions and break the install.
 */
const PIP_PACKAGES = [
  "tensorrt-cu12-libs==10.16.1.11",
  "nvidia-cuda-runtime-cu12==12.9.79",
  "nvidia-cuda-nvrtc-cu12==12.9.86",
  "nvidia-cublas-cu12==12.9.2.10",
  "nvidia-cudnn-cu12==9.23.1.3",
];

/** Official NVIDIA download pages, kept generic so they stay current. */
const NVIDIA_LINKS = [
  {
    label: "CUDA Toolkit 12.x",
    url: "https://developer.nvidia.com/cuda-toolkit-archive",
  },
  { label: "cuDNN 9.x", url: "https://developer.nvidia.com/cudnn" },
  { label: "TensorRT 10.x", url: "https://developer.nvidia.com/tensorrt" },
];

/** How often to re-run detection while the screen is open. */
const POLL_MS = 2000;

/** Which install route the tabs are showing. */
type Route = "nvidia" | "pip";

function skipConsequence(tier: RuntimeTier): string {
  switch (tier) {
    case "cuda":
      return "Mite will use the CUDA backend for now: it works well, but is roughly 2x slower than TensorRT. You can finish installing TensorRT later from Settings.";
    case "tensor_rt":
      return "The TensorRT runtime is already installed, so Mite will use the fastest path.";
    default:
      return "Mite will run on the CPU for now: much slower, but it always works. You can set up GPU acceleration later from Settings.";
  }
}

function TierRow({
  title,
  hint,
  present,
}: {
  title: string;
  hint: string;
  present: boolean;
}) {
  return (
    <div className="tier-row">
      <div className="tier-row-main">
        <span className="tier-row-title">{title}</span>
        <span className="tier-row-hint">{hint}</span>
      </div>
      <span className={`pill ${present ? "ok" : "warn"}`}>
        {present ? "ready" : "incomplete"}
      </span>
    </div>
  );
}

function CommandBlock({ text, copyText }: { text: string; copyText: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(copyText);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be unavailable; the user can still select the text.
    }
  };
  return (
    <div className="cmd-block">
      <pre className="cmd-code">
        <code>{text}</code>
      </pre>
      <button className="btn btn-ghost btn-sm cmd-copy" onClick={copy}>
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

function ConfirmModal({
  title,
  body,
  confirmLabel,
  busy,
  onConfirm,
  onCancel,
}: {
  title: string;
  body: string;
  confirmLabel: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      className="modal-overlay"
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <div className="modal" role="dialog" aria-modal="true" aria-label={title}>
        <h2 className="modal-title">{title}</h2>
        <p className="modal-body">{body}</p>
        <div className="btn-row modal-actions">
          <button className="btn btn-primary" onClick={onConfirm} disabled={busy}>
            {busy ? "Saving..." : confirmLabel}
          </button>
          <button className="btn btn-ghost" onClick={onCancel} disabled={busy}>
            Back
          </button>
        </div>
      </div>
    </div>
  );
}

export function RuntimeSetup({ status, onClose }: RuntimeSetupProps) {
  const [report, setReport] = useState<DoctorReport | null>(status.doctor);
  const [route, setRoute] = useState<Route>("nvidia");
  const [confirmSkip, setConfirmSkip] = useState(false);
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  // True once the user has picked a tab by hand, so the pip auto-default below
  // never overrides a deliberate choice if it resolves late.
  const routePicked = useRef(false);

  const chooseRoute = useCallback((next: Route) => {
    routePicked.current = true;
    setRoute(next);
  }, []);

  // Default to the pip route when pip is on PATH: that route is a single
  // copy-paste command, so it is the path of least resistance for those users.
  useEffect(() => {
    api
      .pipAvailable()
      .then((available) => {
        if (available && !routePicked.current) setRoute("pip");
      })
      .catch(() => undefined);
  }, []);

  // Live re-check: re-run detection on a short interval, debounced so a slow
  // probe never overlaps the next tick. Stops when the screen unmounts.
  useEffect(() => {
    let active = true;
    const poll = async () => {
      if (inFlight.current) return;
      inFlight.current = true;
      try {
        const fresh = await api.detectRuntime();
        if (active) setReport(fresh);
      } catch {
        // Transient failures are fine; the next tick retries.
      } finally {
        inFlight.current = false;
      }
    };
    poll();
    const handle = setInterval(poll, POLL_MS);
    return () => {
      active = false;
      clearInterval(handle);
    };
  }, []);

  const finish = useCallback(async () => {
    setRecording(true);
    setError(null);
    try {
      await api.recordRuntime();
      onClose();
    } catch (err) {
      setError(String(err));
      setRecording(false);
    }
  }, [onClose]);

  const nvidia = report?.nvidia;
  const gpu: GpuRuntimeStatus | null = report?.gpu_runtime ?? null;
  const tier: RuntimeTier = gpu?.tier ?? "cpu";
  const runtimeFolder = `${status.miteHome}\\nvidia-runtime`;
  // The command parts. Displayed across indented lines for readability, but
  // copied as a single line so it runs in any shell (PowerShell or cmd): no
  // shell-specific line-continuation characters to trip a paste.
  const pipParts = [
    `pip install --target "${runtimeFolder}"`,
    `--extra-index-url ${NVIDIA_PIP_INDEX}`,
    ...PIP_PACKAGES,
  ];
  const pipDisplay = pipParts
    .map((line, index) => (index === 0 ? line : `  ${line}`))
    .join("\n");
  const pipCopy = pipParts.join(" ");

  const openLink = (url: string) => {
    api.openUrl(url).catch((err) => setError(String(err)));
  };

  // No NVIDIA GPU: nothing to install. Confirm CPU and move on.
  if (nvidia && !nvidia.available) {
    return (
      <div className="app-shell">
        <main className="app-main">
          <div className="wizard">
            <div className="wizard-hero">
              <MiteMark className="mark" size="2.75rem" />
              <h1>No NVIDIA GPU detected</h1>
              <p>
                Mite will run on the CPU. It is slower than the GPU path but works
                everywhere, and there is nothing to install.
              </p>
            </div>
            <div className="card">
              {error && <div className="error-text">{error}</div>}
              <div className="btn-row">
                <button
                  className="btn btn-primary"
                  onClick={finish}
                  disabled={recording}
                >
                  {recording ? "Saving..." : "Continue on CPU"}
                </button>
              </div>
            </div>
          </div>
        </main>
      </div>
    );
  }

  const ready = tier === "tensor_rt";

  return (
    <div className="app-shell">
      <main className="app-main">
        <div className="wizard wizard-wide">
          <div className="wizard-hero">
            <MiteMark className="mark" size="2.75rem" />
            <h1>Set up GPU acceleration</h1>
            <p className="wide">
              Mite needs several NVIDIA runtime components to use your GPU. These
              are NVIDIA's own software, and their license does not let us install
              them for you.
            </p>
          </div>

          {/* The primary and skip actions live up top, above the status. */}
          <div className="action-bar">
            <span className="action-bar-text">
              {ready
                ? null
                : "GPU acceleration is optional. You can set it up anytime from Settings."}
            </span>
            <div className="action-bar-buttons">
              {!ready && (
                <button
                  className="btn btn-ghost btn-sm"
                  onClick={() => setConfirmSkip(true)}
                  disabled={recording}
                >
                  Skip for now
                </button>
              )}
              <button
                className="btn btn-primary btn-sm"
                onClick={finish}
                disabled={recording || !ready}
              >
                {recording ? "Saving..." : "Continue"}
              </button>
            </div>
          </div>

          <div className="card">
            <div className="card-title">Status</div>
            {nvidia && (
              <p className="card-sub">
                {nvidia.available
                  ? `${nvidia.gpu_name ?? "NVIDIA GPU"}${
                      nvidia.driver_version
                        ? `, driver ${nvidia.driver_version}`
                        : ""
                    }`
                  : "Detecting your GPU..."}
              </p>
            )}
            <p className="card-sub">
              Install both for the fastest path: TensorRT runs on top of the CUDA
              runtime, so it needs CUDA alongside it. CUDA on its own still works,
              but is roughly 2x slower.
            </p>

            {gpu && (
              <div className="tier-rows">
                <TierRow
                  title="TensorRT runtime"
                  hint="The fastest path."
                  present={gpu.tensorrt.present}
                />
                <TierRow
                  title="CUDA runtime"
                  hint="The required base for TensorRT."
                  present={gpu.cuda.present}
                />
              </div>
            )}
            {gpu && !gpu.builder_present && gpu.tensorrt.present && (
              <p className="card-sub subtle">
                Note: the TensorRT engine builder was not found. A cached engine
                will still run, but building a new one on this machine needs that
                component from NVIDIA's TensorRT package.
              </p>
            )}
          </div>

          <div className="install-routes">
            <div className="tabs" role="tablist">
              <button
                type="button"
                role="tab"
                aria-selected={route === "nvidia"}
                className={`tab${route === "nvidia" ? " active" : ""}`}
                onClick={() => chooseRoute("nvidia")}
              >
                Download from NVIDIA
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={route === "pip"}
                className={`tab${route === "pip" ? " active" : ""}`}
                onClick={() => chooseRoute("pip")}
              >
                Install with pip
              </button>
            </div>

            {route === "nvidia" ? (
              <div className="tab-panel">
                <ol className="dl-steps">
                  {NVIDIA_LINKS.map((link, index) => (
                    <li key={link.url}>
                      <button
                        className="dl-step"
                        onClick={() => openLink(link.url)}
                      >
                        <span className="dl-step-num">{index + 1}</span>
                        <span className="dl-step-label">{link.label}</span>
                        <span className="dl-step-icon" aria-hidden="true">
                          ↗
                        </span>
                      </button>
                    </li>
                  ))}
                </ol>
              </div>
            ) : (
              <div className="tab-panel">
                <CommandBlock text={pipDisplay} copyText={pipCopy} />
              </div>
            )}
          </div>

          {error && <div className="error-text">{error}</div>}
        </div>
      </main>

      {confirmSkip && (
        <ConfirmModal
          title="Skip GPU acceleration?"
          body={`${skipConsequence(tier)} You can come back to this anytime from Settings.`}
          confirmLabel="Skip for now"
          busy={recording}
          onConfirm={finish}
          onCancel={() => setConfirmSkip(false)}
        />
      )}
    </div>
  );
}
