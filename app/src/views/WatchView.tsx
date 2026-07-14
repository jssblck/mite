import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onWatchLog,
  onWatchState,
  type WatchLog,
  type WindowSummary,
} from "../lib/api";
import { WindowCard } from "../components/WindowCard";
import { AnsiLine } from "../components/AnsiLine";

interface WatchViewProps {
  watching: boolean;
  onWatchingChange: (running: boolean) => void;
}

/**
 * A log line with a session-stable identity. Keying by array index would break
 * once the buffer hits its cap: every append then shifts all indices, which
 * defeats AnsiLine's memo and re-parses the whole visible buffer per event.
 */
interface KeyedLog extends WatchLog {
  id: number;
}

function LogView({ logs }: { logs: KeyedLog[] }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [logs]);
  return (
    <div className="log-view" ref={ref}>
      {logs.length === 0 ? (
        <div className="log-empty">Waiting for output...</div>
      ) : (
        logs.map((log) => (
          <div key={log.id} className={`log-line ${log.stream}`}>
            <AnsiLine text={log.line} />
          </div>
        ))
      )}
    </div>
  );
}

export function WatchView({ watching, onWatchingChange }: WatchViewProps) {
  const [windows, setWindows] = useState<WindowSummary[]>([]);
  const [launchingId, setLaunchingId] = useState<number | null>(null);
  const [launched, setLaunched] = useState<WindowSummary | null>(null);
  const [logs, setLogs] = useState<KeyedLog[]>([]);
  const nextLogId = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refreshWindows = useCallback(async () => {
    try {
      const list = await api.listWindows();
      setWindows(list);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  // Keep the list current while the picker is on screen: windows open and close
  // while the user decides. Debounced so a slow enumeration never overlaps the
  // next tick, paused while watching or while the window is hidden.
  useEffect(() => {
    if (watching) return;
    let inFlight = false;
    const tick = async () => {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        await refreshWindows();
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
  }, [watching, refreshWindows]);

  useEffect(() => {
    const logUnlisten = onWatchLog((log) => {
      // The id is minted outside the updater so the updater stays pure.
      const id = nextLogId.current++;
      setLogs((prev) => [...prev.slice(-400), { ...log, id }]);
    });
    const stateUnlisten = onWatchState((state) =>
      onWatchingChange(state.running),
    );
    return () => {
      logUnlisten.then((fn) => fn());
      stateUnlisten.then((fn) => fn());
    };
  }, [onWatchingChange]);

  async function startWatching(target: WindowSummary) {
    if (launchingId != null) return;
    setError(null);
    setLogs([]);
    setLaunchingId(target.id);
    try {
      await api.startWatch(target.id);
      setLaunched(target);
      onWatchingChange(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setLaunchingId(null);
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
    return (
      <div>
        <div className="page-head">
          <p>
            {launched
              ? `Reading "${launched.title || launched.appName}". `
              : ""}
            Hover a word over the target window for its definition. Adjust how
            the overlay runs in Settings.
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
      {error && <div className="error-text">{error}</div>}
      {windows.length === 0 ? (
        <div className="empty-state">
          {loaded ? (
            "No readable windows found. Open the game or app you want to read."
          ) : (
            <>
              <span className="inline-spinner" /> Looking for windows...
            </>
          )}
        </div>
      ) : (
        <div className="window-grid">
          {windows.map((w) => (
            <WindowCard
              key={w.id}
              info={w}
              launching={launchingId === w.id}
              disabled={launchingId != null}
              onSelect={() => startWatching(w)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
