// A crossfading layer's opacity (the page's glow and ground field, the
// player's backdrop, the cover blooms): a Web Animation from wherever the
// layer is now to shown (1) or hidden (0), so a cached image still fades
// over the full duration (a CSS transition would not run if the shown
// state were committed before the hidden one was drawn) and a layer
// interrupted mid-fade never jumps. `instant`: the first appearance is
// at once (a layer there when its stack mounts). `ms` is read when the
// fade starts (0 with reduced motion: at once). The layer's CSS starts it
// hidden (`opacity: 0`).

import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { easing } from "../lib/motion";

export function useLayerFade(
  ref: RefObject<HTMLElement | null>,
  visible: boolean,
  instant: boolean,
  ms: () => number,
) {
  const fade = useRef<Animation | null>(null);
  const appeared = useRef(false);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const target = visible ? 1 : 0;
    const from = Number(getComputedStyle(element).opacity);
    fade.current?.cancel();
    fade.current = null;
    element.style.opacity = String(target);
    const first = visible && !appeared.current;
    if (visible) appeared.current = true;
    const duration = first && instant ? 0 : ms();
    if (duration <= 0 || from === target || typeof element.animate !== "function") return;
    fade.current = element.animate([{ opacity: from }, { opacity: target }], {
      duration: duration * Math.abs(target - from),
      easing: easing("--ease-in-out"),
    });
    // Only when it is to be shown or hidden.
  }, [visible]);
  useEffect(() => () => fade.current?.cancel(), []);
}
