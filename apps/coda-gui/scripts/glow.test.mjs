// `node --test scripts/`: the crossfading glow layers (src/lib/glowLayers.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { failed, initialLayers, loaded, MAX_LAYERS, prune, setUrl } from "../src/lib/glowLayers.ts";

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

test("at most three layers; a shown one is never dropped, the newest cover waits for room", () => {
  let s = setUrl(initialLayers("a"), "b");
  const b = s.latest;
  s = loaded(s, b); // b fades in over a
  s = setUrl(s, "c");
  const c = s.latest;
  s = loaded(s, c); // c fades in over both
  assert.equal(MAX_LAYERS, 3);
  assert.deepEqual(urls(s), ["a", "b", "c"]);
  s = setUrl(s, "d");
  assert.deepEqual(urls(s), ["a", "b", "c"], "no shown layer is dropped");
  assert.equal(s.queued, "d");
  assert.equal(s.latest, -1, "nothing else may fade in meanwhile");
  s = setUrl(s, "e"); // a newer cover takes the queue's place
  assert.equal(s.queued, "e");
  // b's fade has run: a goes, and e gets its place.
  s = prune(s, b);
  assert.deepEqual(urls(s), ["b", "c", "e"]);
  assert.deepEqual(shown(s), ["b", "c"]);
  assert.equal(s.queued, null);
  s = loaded(s, s.latest);
  assert.deepEqual(shown(s), ["b", "c", "e"]);
});

test("a layer still loading is replaced, not queued behind", () => {
  let s = setUrl(initialLayers("a"), "b");
  s = loaded(s, s.latest);
  s = setUrl(s, "c"); // loading
  s = setUrl(s, "d");
  assert.deepEqual(urls(s), ["a", "b", "d"]);
  assert.equal(s.queued, null);
});

test("layers fading out stay until pruned; a new cover fades in over them", () => {
  let s = setUrl(initialLayers("a"), null);
  const hiddenFrom = s.next;
  s = setUrl(s, "b");
  assert.deepEqual(urls(s), ["a", "b"], "a keeps fading out");
  assert.deepEqual(shown(s), []);
  s = loaded(s, s.latest);
  s = prune(s, hiddenFrom);
  assert.deepEqual(urls(s), ["b"]);
  // No cover drops a queued one.
  s = setUrl(setUrl(setUrl(s, "c"), "d"), null);
  assert.equal(s.queued, null);
});
