// A track's cover from our `coda` scheme, or a generated placeholder
// (initials on a gradient of two theme cover colours, picked from the album
// name) when there is none.

import { useState } from "react";
import { coverSrc, type CoverSize } from "../lib/ipc";

interface CoverProps {
  url: string | null;
  /** Album (else title) for the placeholder. */
  name: string;
  size: CoverSize;
  className?: string;
}

function hash(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function initials(name: string): string {
  const words = name.split(/\s+/).filter((w) => /[\p{L}\p{N}]/u.test(w));
  const letters = words.slice(0, 2).map((w) => [...w.replace(/^[^\p{L}\p{N}]+/u, "")][0] ?? "");
  return letters.join("").toUpperCase() || "♪";
}

/** The theme's cover colours (`--cover-1` … `--cover-6`, from palette 1–6). */
const COVER_COLORS = 6;

/** Two different cover colours (1-based) for `name`. */
export function placeholderColors(name: string): [number, number] {
  const h = hash(name);
  const a = h % COVER_COLORS;
  const b = (a + 1 + ((h >>> 8) % (COVER_COLORS - 1))) % COVER_COLORS;
  return [a + 1, b + 1];
}

export function Placeholder({ name, className = "" }: { name: string; className?: string }) {
  const [a, b] = placeholderColors(name);
  return (
    <div
      className={`cover cover-placeholder ${className}`}
      style={{
        background: `linear-gradient(135deg, var(--cover-${a}), var(--cover-${b}))`,
      }}
      aria-hidden="true"
    >
      <span>{initials(name)}</span>
    </div>
  );
}

export function Cover({ url, name, size, className = "" }: CoverProps) {
  // Keyed by URL: a new track's cover gets a fresh error state.
  return <CoverImage key={url ?? ""} url={url} name={name} size={size} className={className} />;
}

function CoverImage({ url, name, size, className }: CoverProps) {
  const [failed, setFailed] = useState(false);
  if (!url || failed) return <Placeholder name={name} className={className} />;
  return (
    <img
      className={`cover ${className}`}
      src={coverSrc(url, size)}
      alt=""
      draggable={false}
      onError={() => setFailed(true)}
    />
  );
}
