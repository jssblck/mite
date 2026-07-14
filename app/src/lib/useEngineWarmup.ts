import { useCallback, useEffect, useRef, useState } from "react";
import { api, onWarmupEvent, onWarmupState } from "./api";

export type EngineWarmupPhase = "idle" | "running" | "ready" | "error";

export interface EngineWarmupState {
  phase: EngineWarmupPhase;
  /**
   * True once any step reported a from-scratch TensorRT compile: the
   * multi-minute case that deserves the "this takes a while" message, as
   * opposed to the few-second cache check every launch performs.
   */
  compiling: boolean;
  /** Learner-facing description of the current step, when known. */
  stepLabel: string | null;
  /** 1-based step position and total, 0 when not yet known. */
  step: number;
  stepCount: number;
  error: string | null;
  /** Start (or join) a warmup run. Safe to call while one is running. */
  run: () => void;
}

/** Learner-facing names for the CLI's session targets. */
function friendlyTarget(target: string): string {
  switch (target) {
    case "detector":
      return "text detector";
    case "recognizer":
      return "text recognizer";
    case "fallback_recognizer":
      return "backup recognizer";
    default:
      return target;
  }
}

/**
 * Drive the one-shot engine warmup (`mite warmup`) and expose its progress.
 *
 * Warmup builds the OCR engines exactly as watch would; on the first run after
 * an install, update, or GPU-tier change that compiles TensorRT engines and
 * takes minutes, while on an ordinary launch it validates the cache in
 * seconds. The app runs it before enabling the Watch tab so that first watch
 * never sits silently on a hidden compile. An error is deliberately
 * non-blocking: watch performs the same preparation itself at startup, so the
 * worst case is the old behavior.
 */
export function useEngineWarmup(): EngineWarmupState {
  const [phase, setPhase] = useState<EngineWarmupPhase>("idle");
  const [compiling, setCompiling] = useState(false);
  const [stepLabel, setStepLabel] = useState<string | null>(null);
  const [step, setStep] = useState(0);
  const [stepCount, setStepCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const targetsRef = useRef<string[]>([]);
  const phaseRef = useRef<EngineWarmupPhase>("idle");
  phaseRef.current = phase;

  useEffect(() => {
    const eventUnlisten = onWarmupEvent((event) => {
      switch (event.event) {
        case "start":
          targetsRef.current = event.targets;
          setStepCount(event.targets.length);
          setStep(event.targets.length > 0 ? 1 : 0);
          break;
        case "build_started": {
          const index = targetsRef.current.indexOf(event.target);
          if (index >= 0) setStep(index + 1);
          if (event.likelyCompile) setCompiling(true);
          setStepLabel(
            event.likelyCompile
              ? `Optimizing the ${friendlyTarget(event.target)} for your GPU`
              : `Loading the ${friendlyTarget(event.target)}`,
          );
          break;
        }
        case "warm_started":
          setStepLabel(`Warming up the ${friendlyTarget(event.target)}`);
          break;
        case "done":
          setStepLabel("Finishing");
          break;
        default:
          break;
      }
    });
    const stateUnlisten = onWarmupState((state) => {
      if (state.running) {
        setPhase("running");
        return;
      }
      if (state.code === 0) {
        setPhase("ready");
        setError(null);
      } else {
        setPhase("error");
        setError(
          state.error?.trim() ||
            `the engine preparation exited with code ${state.code ?? "unknown"}`,
        );
      }
    });
    return () => {
      eventUnlisten.then((fn) => fn());
      stateUnlisten.then((fn) => fn());
    };
  }, []);

  const run = useCallback(() => {
    if (phaseRef.current === "running") return;
    setCompiling(false);
    setStepLabel(null);
    setStep(0);
    setStepCount(0);
    setError(null);
    targetsRef.current = [];
    setPhase("running");
    api.startWarmup().catch((err) => {
      setError(String(err));
      setPhase("error");
    });
  }, []);

  return { phase, compiling, stepLabel, step, stepCount, error, run };
}
