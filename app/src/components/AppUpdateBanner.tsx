import { type AppUpdateState } from "../lib/useAppUpdate";
import { ProgressBar } from "./ProgressBar";

interface AppUpdateBannerProps {
  state: AppUpdateState;
}

/**
 * Priority banner for the app's own update. Shown above everything else so the
 * app updates itself first; once it relaunches into the new version, the engine
 * reconciles to that version's range automatically.
 */
export function AppUpdateBanner({ state }: AppUpdateBannerProps) {
  const { phase, update, install, restart, received, total, error } = state;

  if (phase === "available") {
    return (
      <div className="banner banner-strong">
        <span className="banner-text">
          A new version of Mite is available
          {update ? ` (${update.version})` : ""}. Update the app to stay current.
        </span>
        <button className="btn btn-sm btn-primary" onClick={install}>
          Update Mite
        </button>
      </div>
    );
  }

  if (phase === "downloading") {
    return (
      <div className="banner banner-strong">
        <div className="banner-text stack">
          <span>
            <span className="inline-spinner" /> Downloading and verifying the app
            update...
          </span>
          <ProgressBar received={received} total={total} label="App update" />
        </div>
      </div>
    );
  }

  if (phase === "ready") {
    return (
      <div className="banner banner-strong">
        <span className="banner-text">App update installed. Restart to finish.</span>
        <button className="btn btn-sm btn-primary" onClick={restart}>
          Restart now
        </button>
      </div>
    );
  }

  if (phase === "error" && error) {
    return (
      <div className="banner banner-strong">
        <span className="banner-text">Couldn't update the app: {error}</span>
      </div>
    );
  }

  return null;
}
