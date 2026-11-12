// Pure rules for the `progress` store (no IPC, so `node --test scripts/`
// can test them).

import type { Snapshot } from "./types";

/**
 * Whether `next` hears another queue entry than `previous` (by entry key,
 * so the same song queued twice counts as a change), which makes the last
 * position stale. The first snapshot is not a change.
 */
export function playingEntryChanged(
  previous: Pick<Snapshot, "playingKey"> | null,
  next: Pick<Snapshot, "playingKey">,
): boolean {
  return previous != null && previous.playingKey !== next.playingKey;
}
