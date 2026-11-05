import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";
import "./styles/browse.css";

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
