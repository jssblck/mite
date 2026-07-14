import { useCallback, useEffect, useRef, useState } from "react";
import { api, onWatchState, type AppStatus } from "./lib/api";
import { useAppUpdate } from "./lib/useAppUpdate";
import { useEngineReconcile } from "./lib/useEngineReconcile";
import { useEngineWarmup } from "./lib/useEngineWarmup";
import { MiteMark } from "./components/MiteMark";
import { AppUpdateBanner } from "./components/AppUpdateBanner";
import { EngineReconcileNotice } from "./components/EngineReconcileNotice";
import { EngineWarmupNotice } from "./components/EngineWarmupNotice";
import { SetupWizard } from "./views/SetupWizard";
import { RuntimeSetup } from "./views/RuntimeSetup";
import { Dashboard } from "./views/Dashboard";
import { WatchView } from "./views/WatchView";
import { Settings } from "./views/Settings";

type View = "dashboard" | "watch" | "settings";

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState<View>("dashboard");
  const [watching, setWatching] = useState(false);
  const [runtimeSetup, setRuntimeSetup] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.getStatus());
    } finally {
      setLoading(false);
    }
  }, []);

  // Once the core install is ready, drive the two update clocks: prompt for the
  // app's own signed update first (it takes priority), and silently reconcile the
  // engine to the version this app build wants. Gated on `ready` so neither runs
  // during first-time setup; both no-op under `tauri dev` (no updater runtime).
  const ready = Boolean(status?.cliInstalled && status?.modelsReady);
  const appUpdate = useAppUpdate(ready);
  const engineReconcile = useEngineReconcile(ready, refresh);
  const warmup = useEngineWarmup();
  const warming = warmup.phase === "running";

  // Warm the OCR engines once per launch, after the engine reconcile settles
  // (so a just-downloaded engine is the one that gets warmed). The first run
  // after an install, update, or GPU change compiles TensorRT engines for
  // minutes; every other launch this is a seconds-long cache check. The Watch
  // tab stays disabled while it runs. Explicit reruns happen after the guided
  // GPU setup closes and after a manual engine update in Settings.
  const warmedThisLaunch = useRef(false);
  useEffect(() => {
    if (warmedThisLaunch.current || !ready || watching || runtimeSetup) return;
    if (!engineReconcile.settled) return;
    warmedThisLaunch.current = true;
    warmup.run();
  }, [ready, watching, runtimeSetup, engineReconcile.settled, warmup.run]);

  useEffect(() => {
    refresh();
    api.isWatching().then(setWatching).catch(() => undefined);
  }, [refresh]);

  // First-run runtime handling. Once the core install is ready, decide whether
  // the guided NVIDIA setup is needed: open it when an NVIDIA GPU is present but
  // not yet at the TensorRT tier; on a machine with no NVIDIA GPU, silently
  // record CPU so launches stay clean and the flow never nags.
  useEffect(() => {
    if (!status || status.runtimeSetupSeen) return;
    const doctor = status.doctor;
    if (!doctor) return;
    if (!doctor.nvidia.available) {
      api.recordRuntime().then(refresh).catch(() => undefined);
      return;
    }
    if ((doctor.gpu_runtime?.tier ?? "cpu") !== "tensor_rt") {
      setRuntimeSetup(true);
    }
  }, [status, refresh]);

  useEffect(() => {
    const unlisten = onWatchState((state) => setWatching(state.running));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  if (loading) {
    return (
      <div className="app-shell">
        <main className="app-main">
          <p className="empty-state">
            <span className="inline-spinner" /> Checking your setup...
          </p>
        </main>
      </div>
    );
  }

  if (!ready || !status) {
    return (
      <SetupWizard
        status={
          status ?? {
            miteHome: "",
            appVersion: "",
            cliInstalled: false,
            cliVersion: null,
            modelsReady: false,
            runtimeSetupSeen: false,
            doctor: null,
          }
        }
        onDone={refresh}
      />
    );
  }

  if (runtimeSetup) {
    return (
      <RuntimeSetup
        status={status}
        onClose={() => {
          setRuntimeSetup(false);
          refresh();
          // A tier change (say CPU -> TensorRT) means different engines; warm
          // them now rather than during the user's first watch.
          if (!watching) warmup.run();
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <span className="brand">
          <MiteMark className="mark" size="1.35rem" />
          <span lang="ja" role="img" aria-label="Mite" className="brand-name">
            みて
          </span>
        </span>
        <nav className="app-nav">
          <button
            className="nav-btn"
            aria-current={view === "dashboard"}
            onClick={() => setView("dashboard")}
          >
            Dashboard
          </button>
          <button
            className="nav-btn"
            aria-current={view === "watch"}
            disabled={warming && !watching}
            title={
              warming && !watching
                ? "Available once the reading engine is ready"
                : undefined
            }
            onClick={() => setView("watch")}
          >
            Watch
          </button>
        </nav>
        <span className="header-spacer" />
        <span className="header-meta">
          {watching && (
            <span className="pill live">
              <span className="dot" /> Watching
            </span>
          )}
          <span>{status.appVersion}</span>
        </span>
        <button
          className={`icon-btn${view === "settings" ? " active" : ""}`}
          aria-label="Settings"
          aria-current={view === "settings"}
          onClick={() => setView("settings")}
        >
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
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>
      </header>
      <main className="app-main">
        <AppUpdateBanner state={appUpdate} />
        {/* The app update takes priority; only nudge about the engine while no
            app update is pending (after an app update the engine reconciles to
            the new version anyway). */}
        {!appUpdate.pending && <EngineReconcileNotice state={engineReconcile} />}
        <EngineWarmupNotice state={warmup} />
        {view === "dashboard" && (
          <Dashboard
            status={status}
            watching={watching}
            warming={warming}
            onRefresh={refresh}
            onWatch={() => setView("watch")}
            onSetupGpu={() => setRuntimeSetup(true)}
          />
        )}
        {view === "watch" && (
          <WatchView
            watching={watching}
            preparing={warming}
            onWatchingChange={setWatching}
          />
        )}
        {view === "settings" && (
          <Settings
            status={status}
            onRefresh={refresh}
            onOpenRuntimeSetup={() => setRuntimeSetup(true)}
            onEngineUpdated={() => {
              if (!watching) warmup.run();
            }}
          />
        )}
      </main>
    </div>
  );
}

export default App;
