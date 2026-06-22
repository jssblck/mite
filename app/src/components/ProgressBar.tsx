import { formatBytes } from "../lib/format";

interface ProgressBarProps {
  received: number;
  total: number;
  label?: string;
}

/**
 * A determinate bar when the server sent a Content-Length, otherwise an
 * indeterminate sweep. Shows bytes received / total beneath.
 */
export function ProgressBar({ received, total, label }: ProgressBarProps) {
  const pct = total > 0 ? Math.min(100, (received / total) * 100) : null;
  return (
    <div>
      <div className="progress">
        <div
          className={`progress-bar${pct === null ? " indeterminate" : ""}`}
          style={pct === null ? undefined : { width: `${pct}%` }}
        />
      </div>
      <div className="progress-meta">
        <span>{label ?? ""}</span>
        <span>
          {formatBytes(received)}
          {total > 0 ? ` / ${formatBytes(total)}` : ""}
        </span>
      </div>
    </div>
  );
}
