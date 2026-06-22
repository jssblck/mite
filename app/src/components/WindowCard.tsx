import { useEffect, useRef, useState } from "react";
import { api, type WindowSummary } from "../lib/api";

interface WindowCardProps {
  info: WindowSummary;
  selected: boolean;
  onSelect: () => void;
}

/**
 * A picker card showing a live thumbnail of one window, refreshed a couple of
 * times per second. The initial capture is staggered so opening the grid does
 * not capture every window in the same frame.
 */
export function WindowCard({ info, selected, onSelect }: WindowCardProps) {
  const [thumb, setThumb] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    let timer = 0;

    const tick = async () => {
      try {
        const data = await api.captureThumbnail(info.id, 360);
        if (alive.current) setThumb(data);
      } catch {
        // The window may have closed or become uncapturable; keep the last frame.
      }
      if (alive.current) {
        timer = window.setTimeout(tick, 2000);
      }
    };

    const startDelay = window.setTimeout(tick, Math.random() * 600);
    return () => {
      alive.current = false;
      window.clearTimeout(timer);
      window.clearTimeout(startDelay);
    };
  }, [info.id]);

  const label = info.title || info.appName || "Untitled window";

  return (
    <button
      type="button"
      className="window-card"
      aria-selected={selected}
      onClick={onSelect}
    >
      <div className="thumb">
        {thumb ? (
          <img src={thumb} alt="" draggable={false} />
        ) : (
          <span className="thumb-fallback">capturing...</span>
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
