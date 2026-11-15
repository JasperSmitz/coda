// Album genre links on Navidrome: an album's songs carry cleaned genre
// names, but the server matches a genre by its exact value (which can
// have stray spaces). A link is resolved against the server's genre
// listing (cached by `browse.genres`), so it opens what the Genres view
// opens. No React and no IPC here, so `node --test scripts/` can test it.

import type { Genre } from "./types";

export interface GenreLink {
  id: string;
  name: string;
  /** The listing's entry (its counts), when one matched. */
  hint?: Genre;
}

/**
 * Where an album's genre chip leads: the listed genre with the same
 * cleaned name, ignoring case, and its exact server value as the id.
 * Several matches: the one whose value is the cleaned name itself, else
 * the first listed. No match: the chip's own (cleaned) name.
 */
export function resolveGenreLink(
  chip: Pick<Genre, "id" | "name">,
  listing: readonly Genre[],
): GenreLink {
  const key = chip.name.toLowerCase();
  const matches = listing.filter((g) => g.name.toLowerCase() === key);
  const match = matches.find((g) => g.id === chip.name) ?? matches[0];
  return match
    ? { id: match.id, name: match.name, hint: match }
    : { id: chip.id, name: chip.name };
}
