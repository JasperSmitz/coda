// `node --test scripts/`: Search's Top result and genre chips
// (src/lib/search.ts; loaded with Node's built-in type stripping).

import assert from "node:assert/strict";
import { test } from "node:test";
import { genreChips, topResult } from "../src/lib/search.ts";

const artist = (name) => ({ id: name, name, albumCount: 1, trackCount: null, coverUrl: null });
const album = (name) => ({
  id: name,
  name,
  artist: null,
  artistId: null,
  year: null,
  trackCount: null,
  durationMs: null,
  coverUrl: null,
});

test("an exact artist name wins, then an exact album name", () => {
  const artists = [artist("Yes Men"), artist("Yes")];
  const albums = [album("Yes")];
  assert.deepEqual(topResult("yes", artists, albums), { kind: "artist", artist: artists[1] });
  assert.deepEqual(topResult("  YES ", [artist("Yessongs Band")], albums), {
    kind: "album",
    album: albums[0],
  });
});

test("a name starting with the query comes next, artists first", () => {
  const artists = [artist("The Cure"), artist("Genesis")];
  const albums = [album("Genesis Live")];
  assert.equal(topResult("gene", artists, albums).artist, artists[1]);
  assert.equal(topResult("genesis l", artists, albums).album, albums[0]);
});

test("otherwise the backend's first artist, else its first album", () => {
  assert.equal(topResult("xyz", [artist("A")], [album("B")]).artist.name, "A");
  assert.equal(topResult("xyz", [], [album("B")]).album.name, "B");
  assert.equal(topResult("xyz", [], []), null);
});

test("names compare without case, width or extra spacing", () => {
  const artists = [artist("Ｓｉｇｕｒ  Rós")];
  assert.equal(topResult("sigur rós", [artist("Other"), ...artists], []).artist, artists[0]);
});

test("genre chips: the largest genres, A–Z", () => {
  const g = (name, albumCount, trackCount = 0) => ({ id: name, name, albumCount, trackCount });
  const chips = genreChips([g("Rock", 40), g("Jazz", 3), g("Ambient", 12), g("Pop", 12, 9)], 3);
  assert.deepEqual(
    chips.map((c) => c.name),
    ["Ambient", "Pop", "Rock"],
  );
});
