import { type EngineReconcileState } from "../lib/useEngineReconcile";
import { ProgressBar } from "./ProgressBar";

interface EngineReconcileNoticeProps {
  state: EngineReconcileState;
}

/**
 * Passive notice for the silent engine reconcile. The engine downloads itself to
 * match the running app build with no prompt, so this only reports progress while
 * it happens, and an error if it could not (the manual control in Settings is the
 * fallback). It shows nothing when idle or done.
 */
export function EngineReconcileNotice({ state }: EngineReconcileNoticeProps) {
  const { phase, version, received, total, error } = state;

  if (phase === "downloading") {
    return (
      <div className="banner">
        <div className="banner-text stack">
          <span>
            <span className="inline-spinner" /> Updating the mite engine
            {version ? ` to ${version}` : ""}...
          </span>
          <ProgressBar received={received} total={total} label="Engine" />
        </div>
      </div>
    );
  }

  if (phase === "error" && error) {
    return (
      <div className="banner">
        <span className="banner-text">
          Couldn't update the engine automatically: {error}. You can retry from
          Settings.
        </span>
      </div>
    );
  }

  return null;
}
