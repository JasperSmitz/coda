// A small right-click menu. Closes on Escape, outside click, blur or
// scroll; arrow keys move between items.

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";

export interface MenuState {
  x: number;
  y: number;
  key: string;
}

export interface MenuItem {
  label: string;
  disabled?: boolean;
  action: () => void;
}

interface Props {
  state: MenuState;
  items: MenuItem[];
  onClose: () => void;
}

export function ContextMenu({ state, items, onClose }: Props) {
  const menu = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: state.x, top: state.y });

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const el = menu.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos({
      left: Math.min(state.x, window.innerWidth - width - 4),
      top: Math.min(state.y, window.innerHeight - height - 4),
    });
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [state.x, state.y]);

  useEffect(() => {
    const close = (e: Event) => {
      if (e.target instanceof Node && menu.current?.contains(e.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  const onKeyDown = (e: KeyboardEvent) => {
    const buttons = Array.from(
      menu.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [],
    );
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") buttons[(at + 1) % buttons.length]?.focus();
    else if (e.key === "ArrowUp") buttons[(at - 1 + buttons.length) % buttons.length]?.focus();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <div className="context-menu" ref={menu} role="menu" style={pos} onKeyDown={onKeyDown}>
      {items.map((item) => (
        <button
          key={item.label}
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            onClose();
            item.action();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
