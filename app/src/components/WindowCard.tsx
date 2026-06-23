import { type WindowSummary } from "../lib/api";

interface WindowCardProps {
  info: WindowSummary;
  launching: boolean;
  disabled: boolean;
  onSelect: () => void;
}

/**
 * A picker card showing a thumbnail of one window. The thumbnail is captured by
 * the CLI as part of the window listing (see lib/api `listWindows`) and refreshes
 * whenever the list does, so the card just renders what it is handed. Clicking it
 * starts watching that window immediately.
 */
export function WindowCard({
  info,
  launching,
  disabled,
  onSelect,
}: WindowCardProps) {
  const label = info.title || info.appName || "Untitled window";

  return (
    <button
      type="button"
      className="window-card"
      onClick={onSelect}
      disabled={disabled}
    >
      <div className="thumb">
        {info.thumbnail ? (
          <img src={info.thumbnail} alt="" draggable={false} />
        ) : (
          <span className="thumb-fallback">No preview</span>
        )}
        <div className="thumb-hover">
          <span className="thumb-cta">Watch</span>
        </div>
        {launching && (
          <div className="thumb-launching">
            <span className="inline-spinner" /> Starting...
          </div>
        )}
      </div>
      <div className="window-meta">
        <div className="window-title" title={label}>
          {label}
        </div>
        <div className="window-detail">
          {info.width}x{info.height} &middot; pid {info.pid}
        </div>
      </div>
    </button>
  );
}
