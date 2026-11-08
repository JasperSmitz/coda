/** `m:ss`, or `h:mm:ss` from an hour. */
export function formatTime(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "–:––";
  const total = Math.floor(ms / 1000);
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

export function artistLine(artist: string | null, album: string | null): string {
  return [artist, album].filter(Boolean).join(" — ");
}
