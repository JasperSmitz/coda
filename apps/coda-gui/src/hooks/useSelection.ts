// Multi-select over a list of keys: click selects one, Cmd/Ctrl-click
// toggles, Shift-click selects the range from the anchor; ↑/↓ move the
// cursor (Shift extends). Keys that disappear drop out of the selection.

import { useCallback, useEffect, useMemo, useState } from "react";

export interface Selection {
  /** Selected keys. */
  keys: ReadonlySet<string>;
  /** The cursor: the last key clicked or moved to (keyboard focus row). */
  cursor: string | null;
  has: (key: string) => boolean;
  /** The selected keys in list order. */
  ordered: () => string[];
  /** A click with its modifiers. */
  click: (key: string, e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => void;
  /** Moves the cursor to `index` (Shift: extends from the anchor). */
  moveTo: (index: number, extend: boolean) => void;
  /** Selects exactly `keys` (the first one becomes cursor and anchor). */
  set: (keys: string[]) => void;
  selectAll: () => void;
  clear: () => void;
}

export function useSelection(list: string[]): Selection {
  const [keys, setKeys] = useState<ReadonlySet<string>>(new Set());
  const [cursor, setCursor] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<string | null>(null);
  const index = useMemo(() => new Map(list.map((k, i) => [k, i])), [list]);

  // Drop keys that left the list (removed entries).
  useEffect(() => {
    setKeys((current) => {
      let gone = false;
      current.forEach((k) => {
        if (!index.has(k)) gone = true;
      });
      return gone ? new Set([...current].filter((k) => index.has(k))) : current;
    });
    setCursor((c) => (c !== null && !index.has(c) ? null : c));
    setAnchor((a) => (a !== null && !index.has(a) ? null : a));
  }, [index]);

  const range = useCallback(
    (from: string | null, to: string): Set<string> => {
      const a = from !== null ? (index.get(from) ?? index.get(to)!) : index.get(to)!;
      const b = index.get(to)!;
      const [lo, hi] = a <= b ? [a, b] : [b, a];
      return new Set(list.slice(lo, hi + 1));
    },
    [index, list],
  );

  const click = useCallback<Selection["click"]>(
    (key, e) => {
      if (!index.has(key)) return;
      if (e.shiftKey) {
        setKeys(range(anchor, key));
      } else if (e.metaKey || e.ctrlKey) {
        setKeys((current) => {
          const next = new Set(current);
          if (next.has(key)) next.delete(key);
          else next.add(key);
          return next;
        });
        setAnchor(key);
      } else {
        setKeys(new Set([key]));
        setAnchor(key);
      }
      setCursor(key);
    },
    [anchor, index, range],
  );

  const moveTo = useCallback<Selection["moveTo"]>(
    (i, extend) => {
      if (list.length === 0) return;
      const key = list[Math.min(list.length - 1, Math.max(0, i))];
      if (extend) {
        setKeys(range(anchor ?? cursor, key));
        if (anchor === null) setAnchor(cursor ?? key);
      } else {
        setKeys(new Set([key]));
        setAnchor(key);
      }
      setCursor(key);
    },
    [anchor, cursor, list, range],
  );

  const set = useCallback((next: string[]) => {
    setKeys(new Set(next));
    setCursor(next[0] ?? null);
    setAnchor(next[0] ?? null);
  }, []);

  return {
    keys,
    cursor,
    has: (key) => keys.has(key),
    ordered: () => list.filter((k) => keys.has(k)),
    click,
    moveTo,
    set,
    selectAll: () => {
      setKeys(new Set(list));
      // Shift+↑/↓ afterwards extends from the first row.
      setAnchor(list[0] ?? null);
      setCursor((c) => c ?? list[list.length - 1] ?? null);
    },
    clear: () => {
      setKeys(new Set());
      setAnchor(null);
    },
  };
}
