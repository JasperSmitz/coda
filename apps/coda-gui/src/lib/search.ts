// Search's Top result (DESIGN.md "Search"): the best album or artist
// match for the query, from the results the backend ranked. Pure (no
// React/DOM), tested by `scripts/search.test.mjs`.

import type { Album, Artist, Genre } from "./types";

export type TopResult = { kind: "artist"; artist: Artist } | { kind: "album"; album: Album };

/** Case-, width- and spacing-insensitive form of a name or query. */
function fold(text: string): string {
  return text.normalize("NFKC").toLocaleLowerCase().trim().replace(/\s+/g, " ");
}

/**
 * The best match: an artist, then an album, named exactly as the query;
 * then one whose name starts with it (artist first); else the first
 * artist, else the first album, as the backend listed them. `null`
 * without either.
 */
export function topResult(query: string, artists: Artist[], albums: Album[]): TopResult | null {
  const q = fold(query);
  const artist = (match: (name: string) => boolean) => artists.find((a) => match(fold(a.name)));
  const album = (match: (name: string) => boolean) => albums.find((a) => match(fold(a.name)));
  const pick = (a: Artist | undefined, b: Album | undefined): TopResult | null =>
    a ? { kind: "artist", artist: a } : b ? { kind: "album", album: b } : null;
  if (q !== "") {
    const exact = pick(
      artist((n) => n === q),
      album((n) => n === q),
    );
    if (exact) return exact;
    const prefix = pick(
      artist((n) => n.startsWith(q)),
      album((n) => n.startsWith(q)),
    );
    if (prefix) return prefix;
  }
  return pick(artists[0], albums[0]);
}

/**
 * The genre chips of an empty search: the `max` largest genres (by album,
 * then track count), A–Z.
 */
export function genreChips(genres: Genre[], max: number): Genre[] {
  const size = (g: Genre) => [g.albumCount ?? 0, g.trackCount ?? 0];
  return [...genres]
    .sort((a, b) => {
      const [aa, at] = size(a);
      const [ba, bt] = size(b);
      return ba - aa || bt - at;
    })
    .slice(0, max)
    .sort((a, b) => a.name.localeCompare(b.name));
}
