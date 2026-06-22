import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  type AppStatus,
  type DoctorReport,
  type GpuRuntimeStatus,
  type RuntimeTier,
  type TierStatus,
} from "../lib/api";
import { MiteMark } from "../components/MiteMark";

interface RuntimeSetupProps {
  status: AppStatus;
  onClose: () => void;
}

/** The pinned NVIDIA packages, surfaced in the pip route's copy-paste command. */
const PIP_PACKAGES = [
  "tensorrt-cu12==10.16.1.11",
  "nvidia-cuda-runtime-cu12==12.9.79",
  "nvidia-cuda-nvrtc-cu12==12.9.86",
  "nvidia-cublas-cu12==12.9.2.10",
  "nvidia-cudnn-cu12==9.23.1.3",
];

/** Official NVIDIA download pages, kept generic so they stay current. */
const NVIDIA_LINKS = [
  {
    label: "CUDA Toolkit (pick a 12.x release)",
    url: "https://developer.nvidia.com/cuda-toolkit-archive",
    note: "Provides the CUDA runtime, cuBLAS, and NVRTC.",
  },
  {
    label: "cuDNN 9.x",
    url: "https://developer.nvidia.com/cudnn",
    note: "Free NVIDIA developer account required to download.",
  },
  {
    label: "TensorRT 10.x",
    url: "https://developer.nvidia.com/tensorrt",
    note: "Free NVIDIA developer account required to download.",
  },
];

/** How often to re-run detection while the screen is open. */
const POLL_MS = 2000;

function tierName(tier: RuntimeTier): string {
  switch (tier) {
    case "tensor_rt":
      return "TensorRT (fastest)";
    case "cuda":
      return "CUDA (no TensorRT yet)";
    default:
      return "CPU only";
  }
}

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

function Checklist({ title, hint, tier }: { title: string; hint: string; tier: TierStatus }) {
  return (
    <div className="dll-group">
      <div className="dll-group-head">
        <span className="dll-group-title">{title}</span>
        <span className={`pill ${tier.present ? "ok" : "pending"}`}>
          {tier.present ? "ready" : "incomplete"}
        </span>
      </div>
      <p className="dll-group-hint">{hint}</p>
      <ul className="dll-list">
        {tier.components.map((component) => (
          <li
            key={component.name}
            className={`dll-item${component.present ? " present" : ""}`}
          >
            <span className="dll-mark">{component.present ? "✓" : "○"}</span>
            <code>{component.name}</code>
          </li>
        ))}
      </ul>
    </div>
  );
}

function CommandBlock({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be unavailable; the user can still select the text.
    }
  };
  return (
    <div className="cmd-block">
      <code>{text}</code>
      <button className="btn btn-ghost btn-sm" onClick={copy}>
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

export function RuntimeSetup({ status, onClose }: RuntimeSetupProps) {
  const [report, setReport] = useState<DoctorReport | null>(status.doctor);
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);

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
  const pipCommand = `pip install --target "${runtimeFolder}" ${PIP_PACKAGES.join(" ")}`;

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

  return (
    <div className="app-shell">
      <main className="app-main">
        <div className="wizard wizard-wide">
          <div className="wizard-hero">
            <MiteMark className="mark" size="2.75rem" />
            <h1>Set up GPU acceleration</h1>
            <p>
              Mite needs several NVIDIA runtime components to use your GPU.
              Because of NVIDIA's license terms we do not install them for you;
              you install them directly from NVIDIA, and Mite detects them and
              launches with the right settings. The steps below walk you through
              it.
            </p>
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
              Detected runtime: <strong>{tierName(tier)}</strong>. This screen
              re-checks every couple of seconds, so each component below checks
              off as you install it.
            </p>

            {gpu && (
              <div className="dll-groups">
                <Checklist
                  title="TensorRT runtime (fastest path)"
                  hint="The fastest backend. Needs the TensorRT 10.x DLLs plus the CUDA set below."
                  tier={gpu.tensorrt}
                />
                <Checklist
                  title="CUDA runtime"
                  hint="The CUDA 12 runtime, cuBLAS, and cuDNN 9. On its own this enables the CUDA backend (about 2x slower than TensorRT)."
                  tier={gpu.cuda}
                />
              </div>
            )}
            {gpu && !gpu.builder_present && gpu.tensorrt.present && (
              <p className="card-sub subtle">
                Note: nvinfer_builder_resource.dll was not found. A cached engine
                will still run, but building a new TensorRT engine on this
                machine needs that component from NVIDIA's TensorRT package.
              </p>
            )}
          </div>

          <div className="card">
            <div className="card-title">How to install (choose one route)</div>

            <div className="route">
              <div className="route-title">Option A: download from NVIDIA</div>
              <p className="card-sub">
                Install compatible major versions: CUDA 12.x, cuDNN 9.x, and
                TensorRT 10.x. Newer majors will not load.
              </p>
              <ul className="link-list">
                {NVIDIA_LINKS.map((link) => (
                  <li key={link.url}>
                    <button
                      className="link-btn"
                      onClick={() => openLink(link.url)}
                    >
                      {link.label}
                    </button>
                    <span className="link-note">{link.note}</span>
                  </li>
                ))}
              </ul>
            </div>

            <div className="route">
              <div className="route-title">Option B: install with pip</div>
              <p className="card-sub">
                If you have Python, install NVIDIA's official wheels (the exact
                versions Mite is built against) into a folder Mite watches. The
                DLLs land under that folder; Mite finds them automatically.
              </p>
              <CommandBlock text={pipCommand} />
              <p className="card-sub subtle">
                Installs into {runtimeFolder}
              </p>
            </div>
          </div>

          {error && <div className="error-text">{error}</div>}

          <div className="card">
            <p className="card-sub">{skipConsequence(tier)}</p>
            <div className="btn-row">
              <button
                className="btn btn-primary"
                onClick={finish}
                disabled={recording || tier !== "tensor_rt"}
              >
                {recording
                  ? "Saving..."
                  : tier === "tensor_rt"
                    ? "Continue"
                    : "Waiting for TensorRT..."}
              </button>
              <button
                className="btn btn-ghost"
                onClick={finish}
                disabled={recording}
              >
                Skip for now
              </button>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}
