// A track's cover from our `coda` scheme, or a generated placeholder
// (initials on a gradient derived from the album name) when there is none.

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

export function Placeholder({ name, className = "" }: { name: string; className?: string }) {
  const h = hash(name);
  const a = h % 360;
  const b = (a + 40 + ((h >> 9) % 60)) % 360;
  return (
    <div
      className={`cover cover-placeholder ${className}`}
      style={{
        background: `linear-gradient(135deg, hsl(${a} 32% 34%), hsl(${b} 38% 22%))`,
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
