import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onWatchLog,
  onWatchState,
  type WatchLog,
  type WindowSummary,
} from "../lib/api";
import { WindowCard } from "../components/WindowCard";

interface WatchViewProps {
  watching: boolean;
  onWatchingChange: (running: boolean) => void;
}

function LogView({ logs }: { logs: WatchLog[] }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [logs]);
  return (
    <div className="log-view" ref={ref}>
      {logs.length === 0 ? (
        <div className="log-empty">Waiting for output...</div>
      ) : (
        logs.map((log, index) => (
          <div key={index} className={`log-line ${log.stream}`}>
            {log.line}
          </div>
        ))
      )}
    </div>
  );
}

export function WatchView({ watching, onWatchingChange }: WatchViewProps) {
  const [windows, setWindows] = useState<WindowSummary[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [auto, setAuto] = useState(true);
  const [hud, setHud] = useState(false);
  const [metrics, setMetrics] = useState(0);
  const [logs, setLogs] = useState<WatchLog[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refreshWindows = useCallback(async () => {
    setLoading(true);
    try {
      const list = await api.listWindows();
      setWindows(list);
      setSelected((current) =>
        current != null && list.some((w) => w.id === current) ? current : null,
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refreshWindows();
  }, [refreshWindows]);

  useEffect(() => {
    const logUnlisten = onWatchLog((log) =>
      setLogs((prev) => [...prev.slice(-400), log]),
    );
    const stateUnlisten = onWatchState((state) => onWatchingChange(state.running));
    return () => {
      logUnlisten.then((fn) => fn());
      stateUnlisten.then((fn) => fn());
    };
  }, [onWatchingChange]);

  async function start() {
    if (selected == null) return;
    setError(null);
    setLogs([]);
    try {
      await api.startWatch({
        windowId: selected,
        auto,
        hud,
        metricsIntervalSecs: metrics,
      });
      onWatchingChange(true);
    } catch (err) {
      setError(String(err));
    }
  }

  async function stop() {
    try {
      await api.stopWatch();
    } catch (err) {
      setError(String(err));
    }
  }

  if (watching) {
    const target = windows.find((w) => w.id === selected);
    return (
      <div>
        <div className="page-head">
          <p>
            {target ? `Reading "${target.title || target.appName}". ` : ""}
            {auto
              ? "The overlay is active continuously. Hover a word over the target window for its definition."
              : "Hold Shift over the target window to read; hover a word for its definition."}
          </p>
        </div>
        <div className="card">
          <div className="card-title">Engine output</div>
          <LogView logs={logs} />
          {error && <div className="error-text">{error}</div>}
          <div className="btn-row">
            <button className="btn btn-danger" onClick={stop}>
              Stop watching
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="page-head">
        <p>
          Pick the game or app you want to read. Its live preview updates so you
          can confirm the right window before you start.
        </p>
      </div>

      <div className="picker-toolbar">
        <button
          className="btn btn-ghost btn-sm"
          onClick={refreshWindows}
          disabled={loading}
        >
          {loading ? <span className="inline-spinner" /> : "Refresh windows"}
        </button>
        <span className="header-meta">{windows.length} windows</span>
      </div>

      {windows.length === 0 ? (
        <div className="empty-state">
          {loading ? "Looking for windows..." : "No capturable windows found."}
        </div>
      ) : (
        <div className="window-grid">
          {windows.map((w) => (
            <WindowCard
              key={w.id}
              info={w}
              selected={selected === w.id}
              onSelect={() => setSelected(w.id)}
            />
          ))}
        </div>
      )}

      <div className="card" style={{ marginTop: "1.25rem" }}>
        <div className="card-title">Options</div>
        <div className="option-row">
          <input
            id="opt-auto"
            type="checkbox"
            checked={auto}
            onChange={(e) => setAuto(e.target.checked)}
          />
          <label htmlFor="opt-auto">
            Run continuously{" "}
            <span className="hint">(recommended; some games intercept Shift)</span>
          </label>
        </div>
        <div className="option-row">
          <input
            id="opt-hud"
            type="checkbox"
            checked={hud}
            onChange={(e) => setHud(e.target.checked)}
          />
          <label htmlFor="opt-hud">
            Show latency HUD <span className="hint">(per-stage timings)</span>
          </label>
        </div>
        <div className="option-row">
          <input
            id="opt-metrics"
            className="number-input"
            type="number"
            min={0}
            value={metrics}
            onChange={(e) => setMetrics(Math.max(0, Number(e.target.value) || 0))}
          />
          <label htmlFor="opt-metrics">
            Log metrics every N seconds <span className="hint">(0 = off)</span>
          </label>
        </div>

        {error && <div className="error-text">{error}</div>}
        <div className="btn-row">
          <button className="btn btn-primary" onClick={start} disabled={selected == null}>
            Start watching
          </button>
        </div>
      </div>
    </div>
  );
}
