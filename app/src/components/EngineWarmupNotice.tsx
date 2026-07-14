import { type EngineWarmupState } from "../lib/useEngineWarmup";

interface EngineWarmupNoticeProps {
  state: EngineWarmupState;
}

/**
 * Progress banner for the one-shot engine warmup that runs at launch and after
 * engine or GPU-runtime changes. The quick every-launch cache check shows only
 * a brief spinner line; once a step reports a real TensorRT compile the banner
 * explains the wait, because that case takes minutes and looks like a hang
 * otherwise. Warmup has no byte-level progress, so the bar is indeterminate.
 *
 * Errors are surfaced but deliberately do not block watching: watch performs
 * the same preparation itself when it starts.
 */
export function EngineWarmupNotice({ state }: EngineWarmupNoticeProps) {
  const { phase, compiling, stepLabel, step, stepCount, error, run } = state;

  if (phase === "running") {
    return (
      <div className="banner">
        <div className="banner-text stack">
          <span>
            <span className="inline-spinner" /> Preparing the reading engine
            {stepCount > 1 && step > 0 ? ` (step ${step} of ${stepCount})` : ""}
            ...
          </span>
          {compiling && (
            <span>
              Mite is optimizing its reading models for your graphics card.
              This happens once after installs and updates and can take several
              minutes; you can leave this window in the background.
            </span>
          )}
          <div className="progress">
            <div className="progress-bar indeterminate" />
          </div>
          {stepLabel && (
            <span className="progress-meta">
              <span>{stepLabel}...</span>
            </span>
          )}
        </div>
      </div>
    );
  }

  if (phase === "error" && error) {
    return (
      <div className="banner">
        <div className="banner-text stack">
          <span>
            Couldn't prepare the reading engine: {error}. Watching still works;
            it will finish the preparation itself when it starts.
          </span>
          <div className="btn-row">
            <button className="btn btn-ghost btn-sm" onClick={run}>
              Try again
            </button>
          </div>
        </div>
      </div>
    );
  }

  return null;
}
