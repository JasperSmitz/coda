// `node --test scripts/`: the crossfading glow layers (src/lib/glowLayers.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { failed, initialLayers, loaded, prune, setUrl } from "../src/lib/glowLayers.ts";

const shown = (s) => s.layers.filter((l) => l.shown).map((l) => l.url);
const urls = (s) => s.layers.map((l) => l.url);

test("a new cover fades in over the old, which goes after the fade", () => {
  let s = initialLayers("a");
  assert.deepEqual(shown(s), ["a"]);
  s = setUrl(s, "b");
  assert.deepEqual(urls(s), ["a", "b"]);
  assert.deepEqual(shown(s), ["a"]);
  const b = s.latest;
  s = loaded(s, b);
  assert.deepEqual(shown(s), ["a", "b"]);
  s = prune(s, b);
  assert.deepEqual(urls(s), ["b"]);
});

test("no cover hides every layer, pruned after the fade", () => {
  let s = setUrl(initialLayers("a"), null);
  assert.deepEqual(urls(s), ["a"]);
  assert.deepEqual(shown(s), []);
  s = prune(s, s.next);
  assert.deepEqual(urls(s), []);
});

test("the newest cover failing fades the older ones out (an album without art keeps no glow)", () => {
  let s = setUrl(initialLayers("a"), "b");
  const b = s.latest;
  s = failed(s, b);
  assert.deepEqual(urls(s), ["a"]);
  assert.deepEqual(shown(s), [], "a fades out");
  assert.equal(s.latest, -1);
  s = prune(s, b + 1);
  assert.deepEqual(urls(s), []);
});

test("an older layer failing changes nothing else", () => {
  let s = setUrl(initialLayers("a"), "b");
  const b = s.latest;
  s = setUrl(s, "c"); // b, still loading, is replaced
  assert.deepEqual(urls(s), ["a", "c"]);
  s = failed(s, b);
  assert.deepEqual(shown(s), ["a"]);
  s = loaded(s, s.latest);
  assert.deepEqual(shown(s), ["a", "c"]);
});

test("a superseded layer that loads late is not shown", () => {
  let s = setUrl(initialLayers(null), "a");
  const a = s.latest;
  s = setUrl(s, null);
  s = loaded(s, a);
  assert.deepEqual(shown(s), []);
  // A cover added after a failure is not pruned with the old ones.
  s = setUrl(initialLayers("a"), "b");
  const b = s.latest;
  s = failed(s, b);
  s = setUrl(s, "c");
  s = prune(s, b + 1);
  assert.deepEqual(urls(s), ["c"]);
});
