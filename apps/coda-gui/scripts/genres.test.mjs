// `node --test scripts/`: album genre links (src/lib/genres.ts; loaded
// with Node's built-in type stripping).

import assert from "node:assert/strict";
import { test } from "node:test";
import { resolveGenreLink } from "../src/lib/genres.ts";

const genre = (id, name = id.trim()) => ({ id, name, albumCount: 1, trackCount: 2 });

test("an album chip takes the listed genre's exact server value", () => {
  const listing = [genre("Ambient"), genre("Rock ")];
  const link = resolveGenreLink({ id: "Rock", name: "Rock" }, listing);
  assert.equal(link.id, "Rock ");
  assert.equal(link.name, "Rock");
  assert.equal(link.hint, listing[1]);
});

test("the match ignores case", () => {
  const listing = [genre("rock")];
  assert.equal(resolveGenreLink({ id: "Rock", name: "Rock" }, listing).id, "rock");
});

test("several matches: the value equal to the name, else the first listed", () => {
  const listing = [genre(" Rock"), genre("Rock "), genre("Rock")];
  assert.equal(resolveGenreLink({ id: "Rock", name: "Rock" }, listing).id, "Rock");
  assert.equal(resolveGenreLink({ id: "rock", name: "rock" }, listing).id, " Rock");
});

test("an unlisted genre keeps the chip's cleaned name", () => {
  const link = resolveGenreLink({ id: "Shoegaze", name: "Shoegaze" }, [genre("Rock ")]);
  assert.deepEqual(link, { id: "Shoegaze", name: "Shoegaze" });
});
