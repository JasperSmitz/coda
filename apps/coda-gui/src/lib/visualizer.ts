// Visualizer display math, ported from `coda_core::visualizer`: smoothing
// (fast attack, bounded decay), the stale fade, and `spreadBands`, which
// interpolates the 7 measured bands into more bars on a log-frequency
// axis with a monotone cubic (no bar rises above both neighbouring bands).

/** Band centres in Hz (`VISUALIZER_BAND_HZ`). */
export const BAND_HZ = [63, 160, 400, 1000, 2500, 6300, 16000] as const;
/** Share of the way to a louder reading covered at once. */
export const ATTACK = 0.7;
/** Full height falls to nothing in about 0.6 s. */
export const DECAY_PER_SEC = 1.6;
/** Longest gap that still counts for the decay (a stall does not drop bars at once). */
const MAX_STEP_MS = 250;
/** Without a reading for this long the bars fall on their own. */
export const STALE_AFTER_MS = 300;

export type Bands = (number | null)[];

function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}

function smooth(shown: number, target: number, stepSec: number): number {
  const t = clamp01(target);
  return t >= shown ? shown + (t - shown) * ATTACK : Math.max(t, shown - DECAY_PER_SEC * stepSec);
}

/** The smoothed bars shown. Times are `performance.now()` milliseconds. */
export class Meter {
  bands: Bands = BAND_HZ.map(() => null);
  private updated: number | null = null;
  private read: number | null = null;

  update(bands: Bands, now: number): void {
    const step = this.step(now);
    this.bands = BAND_HZ.map((_, i) => {
      const target = bands[i];
      return target == null ? null : smooth(this.bands[i] ?? 0, target, step);
    });
    this.updated = now;
    this.read = now;
  }

  /** Falls towards silence once readings stopped. Returns whether anything moved. */
  fade(now: number): boolean {
    if (this.read == null || now - this.read < STALE_AFTER_MS) return false;
    const step = this.step(now);
    let moved = false;
    this.bands = this.bands.map((v) => {
      if (v == null) return null;
      const next = smooth(v, 0, step);
      if (next !== v) moved = true;
      return next;
    });
    this.updated = now;
    return moved;
  }

  reset(): void {
    this.bands = BAND_HZ.map(() => null);
    this.updated = null;
    this.read = null;
  }

  private step(now: number): number {
    return this.updated == null ? 0 : Math.min(MAX_STEP_MS, Math.max(0, now - this.updated)) / 1000;
  }
}

const position = (i: number) => Math.log(BAND_HZ[i]);

/** The bands as a smooth curve at `x` (the log of a frequency). */
export function bandCurve(bands: Bands, x: number): number | null {
  const n = BAND_HZ.length;
  const value = (i: number): number | null => {
    const v = bands[i];
    return v == null ? null : clamp01(v);
  };
  if (!Number.isFinite(x)) return null;
  for (let i = 0; i < n; i++) if (position(i) === x) return value(i);
  let i = -1;
  for (let k = 0; k < n - 1; k++) {
    if (position(k) < x && x < position(k + 1)) i = k;
  }
  if (i < 0) return null;
  const y0 = value(i);
  const y1 = value(i + 1);
  if (y0 == null || y1 == null) return null;
  const width = (k: number) => position(k + 1) - position(k);
  const secant = (k: number): number | null => {
    const a = value(k);
    const b = value(k + 1);
    return a == null || b == null ? null : (b - a) / width(k);
  };
  const slope = (k: number): number => {
    const s0 = k > 0 ? secant(k - 1) : null;
    const s1 = k + 1 < n ? secant(k) : null;
    if (s0 != null && s1 != null) {
      if (s0 * s1 <= 0) return 0;
      const h0 = width(k - 1);
      const h1 = width(k);
      return (3 * (h0 + h1)) / ((2 * h1 + h0) / s0 + (h1 + 2 * h0) / s1);
    }
    return s0 ?? s1 ?? 0;
  };
  const h = width(i);
  const t = (x - position(i)) / h;
  const t2 = t * t;
  const t3 = t2 * t;
  const y =
    (2 * t3 - 3 * t2 + 1) * y0 +
    (t3 - 2 * t2 + t) * h * slope(i) +
    (-2 * t3 + 3 * t2) * y1 +
    (t3 - t2) * h * slope(i + 1);
  return Math.min(Math.max(y0, y1), Math.max(Math.min(y0, y1), y));
}

/** `bars` values evenly spaced on a log-frequency axis from the lowest band to the highest. */
export function spreadBands(bands: Bands, bars: number): (number | null)[] {
  const n = BAND_HZ.length;
  const low = position(0);
  const high = position(n - 1);
  return Array.from({ length: bars }, (_, bar) => {
    const x =
      bar === 0 ? low : bar === bars - 1 ? high : low + ((high - low) * bar) / (bars - 1);
    return bandCurve(bands, x);
  });
}
