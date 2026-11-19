// Fades elements in when `value` changes (not when first shown), without
// remounting them, so focus, scroll positions and state stay. A change
// during the fade starts it again.

import { useLayoutEffect, useRef } from "react";
import { type Duration, animate } from "../lib/motion";

export function useFadeOnChange(
  value: unknown,
  elements: () => (Element | null | undefined)[],
  { duration = "--motion-slow", from = { opacity: 0 } }: { duration?: Duration; from?: Keyframe } = {},
): void {
  // The value last faded to (not a first-run flag: StrictMode runs mount
  // effects twice).
  const shown = useRef(value);
  const running = useRef<Animation[]>([]);
  const get = useRef(elements);
  get.current = elements;
  const start = useRef(from);
  start.current = from;

  useLayoutEffect(() => {
    if (Object.is(shown.current, value)) return;
    shown.current = value;
    for (const animation of running.current) animation.cancel();
    running.current = [];
    for (const element of get.current()) {
      if (!element) continue;
      const animation = animate(element, [start.current, { opacity: 1, transform: "none" }], duration);
      if (animation) running.current.push(animation);
    }
  }, [value, duration]);

  useLayoutEffect(
    () => () => {
      for (const animation of running.current) animation.cancel();
    },
    [],
  );
}
