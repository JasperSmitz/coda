// The platform the webview runs on, for the macOS window chrome only.

/** macOS (WKWebView's user agent). */
export const isMacos = /Macintosh|Mac OS X/.test(navigator.userAgent);

/**
 * Props that make an element and its non-interactive contents drag the
 * window (double-click zooms), on macOS only, where the title bar is
 * hidden (`titlebar.rs`). Buttons, links, inputs and anything focusable
 * inside still work: Tauri's drag script skips them.
 */
export const dragRegion: { "data-tauri-drag-region"?: "deep" } = isMacos
  ? { "data-tauri-drag-region": "deep" }
  : {};
