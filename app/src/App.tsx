import { useCallback, useEffect, useState } from "react";
import { api, onWatchState, type AppStatus } from "./lib/api";
import { MiteMark } from "./components/MiteMark";
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

  const ready = Boolean(status?.cliInstalled && status?.modelsReady);
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
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <span className="brand">
          <MiteMark className="mark" size="1.35rem" /> Mite
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
            onClick={() => setView("watch")}
          >
            Watch
          </button>
          <button
            className="nav-btn"
            aria-current={view === "settings"}
            onClick={() => setView("settings")}
          >
            Settings
          </button>
        </nav>
        <span className="header-spacer" />
        <span className="header-meta">
          {watching && (
            <span className="pill live">
              <span className="dot" /> Watching
            </span>
          )}
          <span>v{status.appVersion}</span>
        </span>
      </header>
      <main className="app-main">
        {view === "dashboard" && (
          <Dashboard
            status={status}
            watching={watching}
            onRefresh={refresh}
            onWatch={() => setView("watch")}
          />
        )}
        {view === "watch" && (
          <WatchView watching={watching} onWatchingChange={setWatching} />
        )}
        {view === "settings" && (
          <Settings
            status={status}
            onRefresh={refresh}
            onOpenRuntimeSetup={() => setRuntimeSetup(true)}
          />
        )}
      </main>
    </div>
  );
}

export default App;
