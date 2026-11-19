// Motion, from the same tokens as the CSS (`--motion*`, `--ease-*` in
// tokens.css): Web Animations for small things, View Transitions for Now
// Playing's layout changes and track changes, and for the player's cover
// morphing into Now Playing's. With reduced motion nothing
// animates: `animate` returns null and `viewTransition` applies the
// change at once.

import { flushSync } from "react-dom";

/** Looked up once; its `matches` follows the setting live. */
let reducedMotionQuery: MediaQueryList | null | undefined;

export function prefersReducedMotion(): boolean {
  if (reducedMotionQuery === undefined) {
    reducedMotionQuery = window.matchMedia?.("(prefers-reduced-motion: reduce)") ?? null;
  }
  return reducedMotionQuery?.matches ?? false;
}

export type Duration =
  | "--motion-fast"
  | "--motion"
  | "--motion-slow"
  | "--motion-rise"
  | "--motion-sink";
export type Easing = "--ease-out" | "--ease-in-out" | "--spring";

const FALLBACK_MS: Record<Duration, number> = {
  "--motion-fast": 120,
  "--motion": 180,
  "--motion-slow": 320,
  "--motion-rise": 500,
  "--motion-sink": 250,
};

const FALLBACK_EASING: Record<Easing, string> = {
  "--ease-out": "cubic-bezier(0.33, 1, 0.68, 1)",
  "--ease-in-out": "cubic-bezier(0.45, 0, 0.55, 1)",
  "--spring": "cubic-bezier(0.34, 1.56, 0.64, 1)",
};

function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** A duration token in milliseconds (0 with reduced motion). */
export function motionMs(name: Duration): number {
  if (prefersReducedMotion()) return 0;
  const match = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(token(name));
  if (!match) return FALLBACK_MS[name];
  const value = Number(match[1]);
  return match[2] === "s" ? value * 1000 : value;
}

/** A plain `ms`/`s` custom property in milliseconds (0 with reduced motion). */
export function cssMs(name: string, fallback: number): number {
  if (prefersReducedMotion()) return 0;
  const match = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(token(name));
  if (!match) return fallback;
  const value = Number(match[1]);
  return match[2] === "s" ? value * 1000 : value;
}

export function easing(name: Easing): string {
  return token(name) || FALLBACK_EASING[name];
}

/**
 * Runs `keyframes` on `element` for the `duration` token, or returns null
 * (nothing animates) with reduced motion or without the API.
 */
export function animate(
  element: Element,
  keyframes: Keyframe[],
  duration: Duration,
  options: Omit<KeyframeAnimationOptions, "duration"> = {},
): Animation | null {
  const ms = motionMs(duration);
  if (ms <= 0 || typeof element.animate !== "function") return null;
  return element.animate(keyframes, { easing: easing("--ease-out"), ...options, duration: ms });
}

// ----- View Transitions -----
// Same-document View Transitions are in Chromium and in Safari 18 (WebKit;
// WKWebView on macOS); whether a given WebKitGTK has them is only known at
// run time, so they are feature-detected. Without them (or with reduced
// motion) the change is applied at once and callers run their own short
// crossfade (`viewTransitionsOn()` tells them which).

/** The method, looked up (lib.dom declares it always present; it is not). */
type StartViewTransition = (update: () => Promise<void>) => ViewTransition;

function starter(): StartViewTransition | null {
  const start = (document as { startViewTransition?: unknown }).startViewTransition;
  return typeof start === "function" ? (start.bind(document) as StartViewTransition) : null;
}

/** View Transitions will run (available and motion not reduced). */
export function viewTransitionsOn(): boolean {
  return starter() !== null && !prefersReducedMotion();
}

export interface ViewTransitionOptions {
  /**
   * Which transition this is, as `data-vt` on <html> while it runs: the
   * CSS names elements (`view-transition-name`) only for their kind.
   */
  kind: string;
  /** `data-vt-dir` on <html> while it runs (a slide's direction). */
  dir?: string;
  /** Awaited after the update (at most a moment: the screen is frozen meanwhile). */
  settle?: () => Promise<void>;
}

/** The transition last started; an older one leaves the attributes alone. */
let latest = 0;

/** A started view transition (or none: then `skip` does nothing). */
export interface TransitionHandle {
  /** Ends its animation at once (the update still runs, if it has not). */
  skip(): void;
}

const NONE: TransitionHandle = { skip() {} };

/**
 * Applies `update` (a React state change) inside a view transition: the
 * old screen is captured, `update` runs under `flushSync`, `settle` is
 * awaited, then the CSS animates between the two. A newer transition
 * skips the running one: its animation jumps to its end state and the new
 * one starts from there. `update` always runs exactly once: inside the
 * transition, or directly when there is none, it is skipped, or it fails.
 */
export function viewTransition(update: () => void, options: ViewTransitionOptions): TransitionHandle {
  const start = viewTransitionsOn() ? starter() : null;
  if (!start) {
    update();
    return NONE;
  }
  const id = ++latest;
  const root = document.documentElement;
  const unmark = () => {
    if (id !== latest) return;
    delete root.dataset.vt;
    delete root.dataset.vtDir;
  };
  root.dataset.vt = options.kind;
  if (options.dir) root.dataset.vtDir = options.dir;
  else delete root.dataset.vtDir;
  let ran = false;
  let transition: ViewTransition;
  try {
    transition = start(async () => {
      if (ran) return;
      ran = true;
      flushSync(update);
      await options.settle?.();
    });
  } catch {
    // Called from an event handler or a React effect: a plain update (no
    // `flushSync` inside React's lifecycle).
    unmark();
    if (!ran) {
      ran = true;
      update();
    }
    return NONE;
  }
  // Skipped (a newer one, a hidden window): `ready` rejects; the update
  // still runs. `finished` follows the update, also when skipped.
  transition.ready.catch(() => {});
  transition.updateCallbackDone.catch(() => {});
  transition.finished
    .catch(() => {})
    .finally(() => {
      // The update failed or never ran (should not happen): apply it now.
      if (!ran) {
        ran = true;
        try {
          update();
        } catch (error) {
          console.warn("view transition update failed", error);
        }
      }
      unmark();
    });
  return { skip: () => transition.skipTransition() };
}

/** Resolves once every `<img>` in `images` is decoded, or after `ms`. */
export function decodedWithin(images: (Element | null | undefined)[], ms: number): Promise<void> {
  const decodes = images
    .filter((img): img is HTMLImageElement => img instanceof HTMLImageElement)
    .map((img) => img.decode().catch(() => {}));
  if (decodes.length === 0) return Promise.resolve();
  return Promise.race([
    Promise.all(decodes).then(() => {}),
    new Promise<void>((resolve) => window.setTimeout(resolve, ms)),
  ]);
}
