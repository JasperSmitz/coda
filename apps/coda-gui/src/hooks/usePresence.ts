// A panel that slides in when opened and out when closed: it stays
// mounted (and `inert`, so nothing in it takes the focus or is announced)
// until its exit has run. Opening it again during the exit turns the same
// animation around from where it is, and the panel keeps its state. Not
// animated when it is first shown, nor with reduced motion (then it simply
// appears and goes).

import { type RefObject, useLayoutEffect, useRef, useState } from "react";
import { type Duration, animate } from "../lib/motion";

interface Options {
  /** How the panel looks when out of view (`opacity`/`transform` only). */
  hidden: Keyframe;
  duration: Duration;
  /** Where the focus goes when it is inside the panel as it starts to leave. */
  onFocusLeaving?: () => void;
}

/** Whether to render the panel; attach `ref` to its root element. */
export function usePresence(
  open: boolean,
  ref: RefObject<HTMLElement | null>,
  { hidden, duration, onFocusLeaving }: Options,
): boolean {
  const [mounted, setMounted] = useState(open);
  if (open && !mounted) setMounted(true);
  /** The animation in flight, and whether it plays toward hidden. */
  const motion = useRef<{ animation: Animation; leaving: boolean; element: HTMLElement } | null>(null);
  // The state last applied (not a first-run flag: StrictMode runs mount
  // effects twice).
  const applied = useRef(open);
  const options = useRef({ hidden, duration, onFocusLeaving });
  options.current = { hidden, duration, onFocusLeaving };

  useLayoutEffect(() => {
    const element = ref.current;
    if (applied.current === open) {
      if (element) element.inert = !open;
      return;
    }
    applied.current = open;
    if (!element) {
      motion.current?.animation.cancel();
      motion.current = null;
      if (!open) setMounted(false);
      return;
    }
    if (!open && element.contains(document.activeElement)) options.current.onFocusLeaving?.();
    element.inert = !open;

    const current = motion.current;
    if (current && current.animation.playState !== "finished") {
      // Turn the one in flight around (no jump).
      if (current.leaving !== !open) {
        current.animation.reverse();
        current.leaving = !open;
      }
      return;
    }
    // A finished exit may still hold the element hidden (its fill) if it
    // is opened again before it unmounted.
    current?.animation.cancel();
    motion.current = null;
    const shown: Keyframe = { opacity: 1, transform: "none" };
    const animation = animate(
      element,
      open ? [options.current.hidden, shown] : [shown, options.current.hidden],
      options.current.duration,
      // Holds the hidden end until the panel is unmounted (either way
      // round, after a reverse).
      { fill: "both" },
    );
    if (!animation) {
      if (!open) setMounted(false);
      return;
    }
    const entry = { animation, leaving: !open, element };
    motion.current = entry;
    // Its own layer while it moves (the queue panel's backdrop blur is
    // costly to repaint every frame).
    element.style.willChange = "transform, opacity";
    animation.addEventListener("finish", () => {
      if (motion.current !== entry) return;
      if (entry.leaving) {
        // Kept (and its hidden fill) until the panel unmounts.
        setMounted(false);
      } else {
        // Shown: drop the effect, so no transform lingers.
        motion.current = null;
        animation.cancel();
        element.style.willChange = "";
      }
    });
  }, [open, ref]);

  // The element went away mid-exit (its parent unmounted it, e.g. Now
  // Playing opened while the queue column left): the exit is over.
  useLayoutEffect(() => {
    const current = motion.current;
    if (open || !current || current.element === ref.current) return;
    current.animation.cancel();
    motion.current = null;
    setMounted(false);
  });

  return mounted;
}
