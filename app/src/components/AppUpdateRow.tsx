import { useAppUpdate } from "../lib/useAppUpdate";
import { ProgressBar } from "./ProgressBar";

interface AppUpdateRowProps {
  appVersion: string;
}

/**
 * Manual self-update control for the desktop app, rendered as a settings row.
 * Shares its lifecycle with the priority banner on the dashboard via
 * `useAppUpdate`; this row is the on-demand entry point ("Check for updates")
 * for when no banner is showing. Separate from the "Update" control in the
 * Engine row, which reinstalls the mite CLI.
 */
export function AppUpdateRow({ appVersion }: AppUpdateRowProps) {
  const { phase, update, error, received, total, check, install, restart } =
    useAppUpdate();

  const summary =
    phase === "available" && update
      ? `Version ${update.version} is available (you have ${appVersion}).`
      : phase === "uptodate"
        ? `You're on the latest version (${appVersion}).`
        : phase === "downloading"
          ? "Downloading and verifying the update..."
          : phase === "ready"
            ? "Update installed. Restart Mite to finish."
            : appVersion;

  return (
    <>
      <div className="setting-row">
        <div className="setting-main">
          <div className="setting-label">App version</div>
          <div className="setting-detail">{summary}</div>
        </div>
        <div className="setting-actions">
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

      {phase === "available" && update?.body && (
        <div className="code-path setting-note">{update.body}</div>
      )}
      {phase === "downloading" && (
        <div className="setting-note">
          <ProgressBar received={received} total={total} label="App update" />
        </div>
      )}
      {error && <div className="error-text setting-note">{error}</div>}
    </>
  );
}
