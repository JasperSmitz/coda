// The queue as a floating panel above the bar, anchored to its right edge
// (`q` or the bar's queue button; 2.0 N1, until the floating player). It
// rises and fades in (`usePresence`; none with reduced motion), takes the
// focus (its list) when opened, and closes on Esc (App, through
// `useBrowseKeys`), `q`, the button, or a pointer-down outside it, unless
// a menu or dialog is open (one may have been opened from the queue) or a
// drag in it is under way. Closing with the focus inside gives it to the
// queue button. It takes the playing cover's colours (useColourScope).

import { useEffect, useRef, type ReactNode } from "react";
import { usePresence } from "../hooks/usePresence";
import { useColourScope } from "../state/palettes";

/** How the panel looks out of view. */
const HIDDEN: Keyframe = { opacity: 0, transform: "translateY(12px)" };

/** The bar's queue button. */
function focusQueueButton() {
  document.querySelector<HTMLElement>(".bar .queue-toggle")?.focus();
}

interface Props {
  open: boolean;
  onClose: () => void;
  /** The playing track's cover, for its colours. */
  coverUrl: string | null;
  children: ReactNode;
}

export function QueuePopover({ open, onClose, coverUrl, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  useColourScope(ref, coverUrl);
  const shown = usePresence(open, ref, {
    hidden: HIDDEN,
    duration: "--motion",
    onFocusLeaving: focusQueueButton,
  });

  // Opened (not restored open at launch): the focus goes to the list.
  const wasOpen = useRef(open);
  useEffect(() => {
    const opened = open && !wasOpen.current;
    wasOpen.current = open;
    if (opened) ref.current?.querySelector<HTMLElement>(".queue-list")?.focus({ preventScroll: true });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target;
      if (!(target instanceof Element)) return;
      const panel = ref.current;
      if (!panel || panel.contains(target)) return;
      // The button toggles it itself.
      if (target.closest(".queue-toggle")) return;
      // A menu or dialog takes this pointer-down (it may be the queue's).
      if (document.querySelector(".context-menu, .dialog-backdrop")) return;
      if (panel.querySelector(".queue-list.is-dragging")) return;
      onClose();
    };
    // Capture: before a menu's own outside-click handler removes it.
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [open, onClose]);

  if (!shown) return null;
  return (
    <div ref={ref} className="queue-popover">
      {children}
    </div>
  );
}
