import React from "react";
import ReactDOM from "react-dom/client";
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
