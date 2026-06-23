import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";

// Shared design language: the marketing site's tokens + base styles are the
// single source of truth (global.css @imports tokens.css). The app layers its
// own component styles on top in app.css.
import "../../site/src/styles/global.css";
import "./styles/app.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// The native window is created hidden (visible:false in tauri.conf.json) and is
// revealed here, only after React has painted its first frame. The first frame
// is the already-dark app shell, so the user never sees the blank white
// default-position window Tauri would otherwise flash before the UI mounts. Two
// animation frames guarantee that first paint has been committed before we show.
function revealWindow(): void {
  getCurrentWindow()
    .show()
    .then(() => getCurrentWindow().setFocus())
    .catch(() => {
      // Not running under the Tauri runtime (e.g. plain browser dev): nothing to
      // reveal, the page is already visible.
    });
}

requestAnimationFrame(() => requestAnimationFrame(revealWindow));
