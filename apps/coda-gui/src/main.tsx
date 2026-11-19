import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { isMacos } from "./lib/platform";
import "./styles/tokens.css";
import "./styles/fonts.css";
import "./styles/base.css";
import "./styles/app.css";
import "./styles/browse.css";
import "./styles/experience.css";
import "./styles/edit.css";
import "./styles/mixes.css";

// macOS hides the window's title bar; `is-macos` makes room for its
// buttons (app.css). Set before the first render.
if (isMacos) document.documentElement.classList.add("is-macos");

// A desktop app, not a page: no browser context menu outside our own.
window.addEventListener("contextmenu", (e) => {
  if (!(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
    e.preventDefault();
  }
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
