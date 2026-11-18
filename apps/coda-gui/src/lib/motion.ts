// Motion for the Web Animations API, from the same tokens as the CSS
// (`--motion*`, `--ease-*` in tokens.css). With reduced motion nothing
// animates: `animate` returns null and callers jump to the end state.

export function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

export type Duration = "--motion-fast" | "--motion" | "--motion-slow";
export type Easing = "--ease-out" | "--ease-in-out";

const FALLBACK_MS: Record<Duration, number> = {
  "--motion-fast": 120,
  "--motion": 180,
  "--motion-slow": 320,
};

const FALLBACK_EASING: Record<Easing, string> = {
  "--ease-out": "cubic-bezier(0.22, 1, 0.36, 1)",
  "--ease-in-out": "cubic-bezier(0.65, 0, 0.35, 1)",
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
