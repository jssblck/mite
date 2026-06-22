import { useState } from "react";
import { appUpdater, type AppUpdate } from "../lib/api";
import { ProgressBar } from "./ProgressBar";

type Phase =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "uptodate"
  | "error";

interface AppUpdateCardProps {
  appVersion: string;
}

/**
 * Self-update control for the desktop app. Checks the release feed, and when a
 * newer signed build exists, downloads and verifies it, then offers a relaunch.
 * This is separate from the "Update engine" control, which updates the mite CLI.
 */
export function AppUpdateCard({ appVersion }: AppUpdateCardProps) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [received, setReceived] = useState(0);
  const [total, setTotal] = useState(0);

  async function check() {
    setPhase("checking");
    setError(null);
    try {
      const found = await appUpdater.check();
      if (found) {
        setUpdate(found);
        setPhase("available");
      } else {
        setPhase("uptodate");
      }
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }

  async function install() {
    if (!update) return;
    setPhase("downloading");
    setError(null);
    setReceived(0);
    setTotal(0);
    let got = 0;
    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            got = 0;
            setReceived(0);
            setTotal(event.data.contentLength ?? 0);
            break;
          case "Progress":
            got += event.data.chunkLength;
            setReceived(got);
            break;
          case "Finished":
            break;
        }
      });
      setPhase("ready");
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }

  async function restart() {
    setError(null);
    try {
      await appUpdater.relaunch();
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }

  const summary =
    phase === "available" && update
      ? `Version ${update.version} is available. You have ${appVersion}.`
      : phase === "uptodate"
        ? `You are on the latest version (${appVersion}).`
        : phase === "downloading"
          ? "Downloading and verifying the update..."
          : phase === "ready"
            ? "Update installed. Restart Mite to finish."
            : `Current version: ${appVersion}. The app updates itself from signed releases.`;

  return (
    <div className="card">
      <div className="card-title">App updates</div>
      <p className="card-sub">{summary}</p>

      {phase === "available" && update?.body && (
        <div className="code-path">{update.body}</div>
      )}

      {phase === "downloading" && (
        <ProgressBar received={received} total={total} label="App update" />
      )}

      {error && <div className="error-text">{error}</div>}

      <div className="btn-row">
        {phase === "ready" ? (
          <button className="btn btn-primary btn-sm" onClick={restart}>
            Restart now
          </button>
        ) : phase === "available" ? (
          <button className="btn btn-primary btn-sm" onClick={install}>
            Download and install
          </button>
        ) : (
          <button
            className="btn btn-ghost btn-sm"
            onClick={check}
            disabled={phase === "checking" || phase === "downloading"}
          >
            {phase === "checking" ? "Checking..." : "Check for updates"}
          </button>
        )}
      </div>
    </div>
  );
}
