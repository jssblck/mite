import { useCallback, useEffect, useState } from "react";
import { api, onWatchState, type AppStatus } from "./lib/api";
import { MiteMark } from "./components/MiteMark";
import { SetupWizard } from "./views/SetupWizard";
import { Dashboard } from "./views/Dashboard";
import { WatchView } from "./views/WatchView";
import { Settings } from "./views/Settings";

type View = "dashboard" | "watch" | "settings";

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState<View>("dashboard");
  const [watching, setWatching] = useState(false);

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
            gpuPackInstalled: false,
            doctor: null,
          }
        }
        onDone={refresh}
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
        {view === "settings" && <Settings status={status} onRefresh={refresh} />}
      </main>
    </div>
  );
}

export default App;
