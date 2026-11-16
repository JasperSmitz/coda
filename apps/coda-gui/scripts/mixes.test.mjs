// `node --test scripts/`: the mix editor's pure logic (src/lib/mixRecipe.ts;
// loaded with Node's built-in type stripping).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_INGREDIENTS,
  activeMixLabel,
  albumIngredient,
  artistIngredient,
  defaultMixName,
  describeIngredient,
  genreIngredient,
  hasIngredient,
  ingredientDetail,
  ingredientKey,
  ingredientLabel,
  mergeSearchResults,
  rereadOutcome,
  withIngredient,
} from "../src/lib/mixRecipe.ts";

const genre = (name) => ({ kind: "genre", name });
const artist = (name, id = null) => ({ kind: "artist", name, id });
const album = (artist, name, id = null) => ({ kind: "album", artist, name, id });

test("chip labels, details and descriptions", () => {
  assert.equal(ingredientLabel(genre(" Prog Rock ")), "Prog Rock");
  assert.equal(ingredientDetail(genre("Rock")), null);
  assert.equal(ingredientLabel(album("Yes", "Relayer")), "Relayer");
  assert.equal(ingredientDetail(album("Yes", "Relayer")), "Yes");
  assert.equal(describeIngredient(album("Yes", "Relayer")), "album Relayer by Yes");
  assert.equal(describeIngredient(artist("Genesis")), "artist Genesis");
  assert.equal(describeIngredient(genre("Jazz")), "genre Jazz");
});

test("local keys ignore case and spaces; Navidrome uses ids and exact genre values", () => {
  assert.equal(ingredientKey(artist("Yes"), "local"), ingredientKey(artist(" yes "), "local"));
  assert.equal(
    ingredientKey(album("Yes", "Relayer"), "local"),
    ingredientKey(album("YES", "relayer"), "local"),
  );
  assert.notEqual(
    ingredientKey(album("Yes", "Relayer"), "local"),
    ingredientKey(album("Yes Relayer", ""), "local"),
  );
  assert.equal(
    ingredientKey(artist("Yes", "ar-1"), "navidrome"),
    ingredientKey(artist("YES (renamed)", "ar-1"), "navidrome"),
  );
  assert.notEqual(
    ingredientKey(artist("Yes", "ar-1"), "navidrome"),
    ingredientKey(artist("Yes", "ar-2"), "navidrome"),
  );
  // A Navidrome genre is its exact value: stray spaces make another genre.
  assert.notEqual(ingredientKey(genre("Rock "), "navidrome"), ingredientKey(genre("Rock"), "navidrome"));
  // Kinds never collide.
  assert.notEqual(ingredientKey(genre("Yes"), "local"), ingredientKey(artist("Yes"), "local"));
});

test("adding keeps the order and drops duplicates", () => {
  const recipe = [genre("Prog Rock"), artist("Yes")];
  assert.deepEqual(withIngredient(recipe, artist("yes"), "local"), recipe);
  assert.deepEqual(withIngredient(recipe, artist("Genesis"), "local"), [...recipe, artist("Genesis")]);
  assert.equal(hasIngredient(recipe, genre("prog rock"), "local"), true);
  assert.equal(hasIngredient(recipe, genre("prog rock"), "navidrome"), false);
  assert.equal(MAX_INGREDIENTS, 50);
});

test("an unnamed recipe is named after its first ingredients", () => {
  assert.equal(defaultMixName([]), "Quick mix");
  assert.equal(defaultMixName([genre("Indie Rock")]), "Indie Rock");
  assert.equal(
    defaultMixName([artist("King Crimson"), artist("Yes"), artist("Genesis")]),
    "King Crimson + Yes + Genesis",
  );
  assert.equal(
    defaultMixName([genre("A"), genre("B"), genre("C"), genre("D")]),
    "A + B + 2 more",
  );
});

test("the active mix's label says whether it is a quick mix", () => {
  assert.equal(activeMixLabel({ name: "Prog", id: "m1", loading: false }), "Mix: Prog");
  assert.equal(activeMixLabel({ name: "Yes", id: null, loading: true }), "Quick mix: Yes");
});

test("browse items become ingredients by name locally, with ids on Navidrome", () => {
  assert.deepEqual(genreIngredient("local", { id: "l3:7", name: "Rock" }), genre("Rock"));
  // Navidrome: the genre's id is the server's exact value.
  assert.deepEqual(genreIngredient("navidrome", { id: "Rock ", name: "Rock" }), genre("Rock "));
  assert.deepEqual(artistIngredient("local", { id: "l3:1", name: "Yes" }), artist("Yes"));
  assert.deepEqual(artistIngredient("navidrome", { id: "ar-1", name: "Yes" }), artist("Yes", "ar-1"));
  assert.deepEqual(
    albumIngredient("navidrome", { id: "al-9", name: "Relayer", artist: " Yes " }),
    album(" Yes ", "Relayer", "al-9"),
    "the album artist is kept exactly as returned",
  );
  assert.deepEqual(
    albumIngredient("local", { id: "l3:2", name: "Relayer", artist: "Yes" }),
    album("Yes", "Relayer"),
  );
  assert.equal(albumIngredient("local", { id: "x", name: "Untitled", artist: null }), null);
  assert.equal(albumIngredient("navidrome", { id: "x", name: "Untitled", artist: "  " }), null);
});

test("search results: genres, artists, albums; prefix matches first; chosen ones left out", () => {
  const groups = {
    genres: [genre("Progressive Rock"), genre("Art Rock"), genre("Rock")],
    artists: [artist("Grateful Dead"), artist("Rockpile"), artist("Yes")],
    albums: [album("Yes", "Rock On"), album("Yes", "Fragile")],
  };
  const merged = mergeSearchResults(groups, "rock", [genre("art rock")], "local");
  assert.deepEqual(
    merged.map((i) => `${i.kind}:${i.name}`),
    [
      "genre:Rock",
      "genre:Progressive Rock",
      "artist:Rockpile",
      "artist:Grateful Dead",
      "artist:Yes",
      "album:Rock On",
      "album:Fragile",
    ],
  );
});

test("search results are capped per kind and never repeat", () => {
  const many = Array.from({ length: 20 }, (_, i) => artist(`Artist ${i}`));
  const merged = mergeSearchResults(
    { genres: [genre("Jazz"), genre("jazz")], artists: many, albums: [] },
    "",
    [],
    "local",
    3,
  );
  assert.deepEqual(
    merged.map((i) => i.name),
    ["Jazz", "Artist 0", "Artist 1", "Artist 2"],
  );
  assert.deepEqual(mergeSearchResults({ genres: [], artists: [], albums: [] }, "x", [], "local"), []);
});

// ----- the editor's search (src/lib/mixSearch.ts) ----------------------------

import { MixSearch, resultsAreCurrent } from "../src/lib/mixSearch.ts";
import { sameRecipe } from "../src/lib/mixRecipe.ts";

/** A promise resolved or rejected from the test. */
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => {
    resolve = ok;
    reject = fail;
  });
  return { promise, resolve, reject };
}

/** Searchers whose replies the test controls, and timers it fires by hand. */
function harness() {
  const calls = [];
  const search = (kind) => (source, query) => {
    const d = deferred();
    calls.push({ kind, source, query, ...d });
    return d.promise;
  };
  const timers = [];
  const states = [];
  const s = new MixSearch(
    { genres: search("genres"), artists: search("artists"), albums: search("albums") },
    (state) => states.push(state),
    200,
    {
      set: (run) => {
        const t = { run, cleared: false };
        timers.push(t);
        return t;
      },
      clear: (t) => {
        t.cleared = true;
      },
    },
  );
  const fire = () => timers.filter((t) => !t.cleared).forEach((t) => ((t.cleared = true), t.run()));
  const reply = async (query, values) => {
    for (const c of calls.filter((c) => c.query === query)) {
      const v = values?.[c.kind];
      if (v instanceof Error) c.reject(v);
      else c.resolve(v ?? []);
    }
    await new Promise((r) => setImmediate(r));
  };
  return { s, calls, states, fire, reply };
}

test("search results are choosable only for the completed search of what is typed now", async () => {
  const h = harness();
  h.s.set("local", " yes ");
  assert.equal(h.s.current.state, "searching");
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes"), false);
  h.fire();
  await h.reply("yes", { artists: [artist("Yes")] });
  assert.equal(h.s.current.state, "done");
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes"), true);
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes "), true, "trimmed");
  assert.equal(resultsAreCurrent(h.s.current, "navidrome", "yes"), false, "other source");

  // Typing on: the old results stay up but are not choosable, during the
  // delay and while the replies are pending.
  h.s.set("local", "yes tor");
  assert.equal(h.s.current.state, "searching");
  assert.deepEqual(h.s.current.groups.artists, [artist("Yes")]);
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes tor"), false);
  h.fire();
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes tor"), false);
  await h.reply("yes tor", { albums: [album("Yes", "Tormato")] });
  assert.equal(resultsAreCurrent(h.s.current, "local", "yes tor"), true);
  assert.deepEqual(h.s.current.groups.albums, [album("Yes", "Tormato")]);
});

test("a newer query wins over a late reply; clearing is idle; nothing after dispose", async () => {
  const h = harness();
  h.s.set("local", "gen");
  h.fire();
  h.s.set("local", "genesis");
  h.fire();
  await h.reply("genesis", { artists: [artist("Genesis")] });
  await h.reply("gen", { artists: [artist("Gentle Giant")] });
  assert.equal(h.s.current.query, "genesis");
  assert.deepEqual(h.s.current.groups.artists, [artist("Genesis")]);

  // The same query again (a trailing space) does not search again.
  const before = h.calls.length;
  h.s.set("local", "genesis ");
  h.fire();
  assert.equal(h.calls.length, before);
  assert.equal(resultsAreCurrent(h.s.current, "local", "genesis "), true);

  h.s.set("local", "  ");
  assert.deepEqual(h.s.current, { state: "idle" });
  assert.equal(resultsAreCurrent(h.s.current, "local", ""), false);

  h.s.set("local", "jazz");
  h.fire();
  h.s.dispose();
  const states = h.states.length;
  await h.reply("jazz", { genres: [genre("Jazz")] });
  assert.equal(h.states.length, states, "a reply after dispose is dropped");
});

test("a query typed during the delay never searches the older one", () => {
  const h = harness();
  h.s.set("local", "y");
  h.s.set("local", "ye");
  h.s.set("local", "yes");
  h.fire();
  assert.deepEqual([...new Set(h.calls.map((c) => c.query))], ["yes"]);
});

test("one failed search names the failure and keeps the others", async () => {
  const h = harness();
  h.s.set("navidrome", "rock");
  h.fire();
  await h.reply("rock", { genres: [genre("Rock")], albums: new Error("server down") });
  assert.equal(h.s.current.failed, "Some results are missing: server down");
  assert.deepEqual(h.s.current.groups.genres, [genre("Rock")]);
  h.s.set("navidrome", "pop");
  h.fire();
  const down = new Error("offline");
  await h.reply("pop", { genres: down, artists: down, albums: down });
  assert.equal(h.s.current.failed, "Could not search: offline");
});

test("sameRecipe: exactly the same, in order", () => {
  const a = [genre("Rock"), artist("Yes", "ar-1"), album("Yes", "Fragile")];
  assert.equal(sameRecipe(a, [genre("Rock"), artist("Yes", "ar-1"), album("Yes", "Fragile", null)]), true);
  assert.equal(sameRecipe(a, [genre("rock"), artist("Yes", "ar-1"), album("Yes", "Fragile")]), false);
  assert.equal(sameRecipe(a, [artist("Yes", "ar-1"), genre("Rock"), album("Yes", "Fragile")]), false);
  assert.equal(sameRecipe(a, a.slice(0, 2)), false);
  assert.equal(sameRecipe(a, [genre("Rock"), artist("Yes"), album("Yes", "Fragile")]), false);
});

test("the editor's opening re-read: never during a save, gone, shown unless touched", () => {
  const listed = { id: "m1", name: "Old", source: "local", ingredients: [genre("Rock")] };
  const fresh = { ...listed, name: "New", ingredients: [genre("Rock"), genre("Jazz")] };
  const other = { id: "m2", name: "Other", source: "local", ingredients: [genre("Pop")] };
  const mixes = [other, fresh];
  assert.deepEqual(rereadOutcome("m1", mixes, { saving: false, touched: false }), {
    kind: "fresh",
    mix: fresh,
    show: true,
  });
  assert.deepEqual(rereadOutcome("m1", mixes, { saving: false, touched: true }), {
    kind: "fresh",
    mix: fresh,
    show: false,
  });
  // A save in flight: what is shown is what is being saved, touched or not.
  assert.deepEqual(rereadOutcome("m1", mixes, { saving: true, touched: false }), { kind: "ignore" });
  assert.deepEqual(rereadOutcome("m1", mixes, { saving: true, touched: true }), { kind: "ignore" });
  assert.deepEqual(rereadOutcome("m1", [other], { saving: false, touched: false }), { kind: "gone" });
  assert.deepEqual(rereadOutcome("m1", [other], { saving: true, touched: false }), { kind: "ignore" });
});
