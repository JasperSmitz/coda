// `node --test scripts/`: the navigation model (src/lib/navModel.ts;
// loaded with Node's built-in type stripping).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  activeRoot,
  back,
  current,
  currentStack,
  effectiveSection,
  goTo,
  initialNav,
  localOrderOf,
  openRootFor,
  push,
  replaceTop,
  resetSource,
  search,
  sectionOf,
  sectionRoute,
  serverSortOf,
  sectionsFor,
  selectRoot,
  setSection,
  setSource,
  toRoot,
  topEntry,
} from "../src/lib/navModel.ts";

const start = (sections = { local: "albums", navidrome: "albums" }, source = "local") =>
  initialNav(sections, source);
const album = (id, source = "local") => ({ kind: "album", source, id, name: id });
const artist = (id, source = "local") => ({ kind: "artist", source, id, name: id });
const kinds = (state) => currentStack(state).map((e) => e.route.kind);

test("every root starts at its own root route, Home shown", () => {
  const s = start({ local: "genres", navidrome: "albums" });
  assert.equal(current(s).root, "home");
  assert.deepEqual(kinds(s), ["home"]);
  assert.deepEqual(kinds(selectRoot(s, "search")), ["search"]);
  assert.deepEqual(kinds(selectRoot(s, "library")), ["genres"]);
  assert.deepEqual(kinds(selectRoot(s, "prefs")), ["settings"]);
});

test("each root keeps its own stack; switching shows it where it was left", () => {
  let s = start();
  s = push(s, album("a")); // Home → Album
  s = selectRoot(s, "library");
  s = push(s, artist("x")); // Albums → Artist
  s = push(s, album("b")); // → Album
  assert.deepEqual(kinds(s), ["albums", "artist", "album"]);
  const libraryTop = topEntry(s);
  s = selectRoot(s, "home");
  assert.deepEqual(kinds(s), ["home", "album"]);
  assert.equal(topEntry(s).route.id, "a");
  s = selectRoot(s, "library");
  assert.equal(topEntry(s), libraryTop, "the same entry (its scroll position) is shown again");
});

test("back pops the active root's stack and does nothing at its root", () => {
  let s = selectRoot(start(), "library");
  s = push(s, artist("x"));
  s = push(s, album("b"));
  s = back(s);
  assert.deepEqual(kinds(s), ["albums", "artist"]);
  s = back(s);
  assert.deepEqual(kinds(s), ["albums"]);
  assert.equal(back(s), null);
  // Home's stack is untouched by Library's pops.
  assert.deepEqual(kinds(selectRoot(s, "home")), ["home"]);
});

test("clicking the active root's entry again pops its stack to the root", () => {
  let s = selectRoot(start(), "library");
  const root = topEntry(s);
  s = push(s, artist("x"));
  s = push(s, album("b"));
  s = selectRoot(s, "library");
  assert.deepEqual(kinds(s), ["albums"]);
  assert.equal(topEntry(s), root, "the root entry itself, not a new one");
  assert.equal(selectRoot(s, "library"), s, "at the root: no change");
});

test("a push from Preferences goes onto Library's stack and shows it", () => {
  let s = selectRoot(start(), "prefs");
  assert.equal(activeRoot(s), "prefs");
  s = push(s, album("a"));
  assert.equal(activeRoot(s), "library");
  assert.deepEqual(kinds(s), ["albums", "album"]);
});

test("leaving Preferences shows the browse root it was opened over", () => {
  let s = push(start(), album("a")); // Home → Album
  s = selectRoot(s, "prefs");
  assert.deepEqual(kinds(s), ["settings"]);
  const left = back(s);
  assert.equal(activeRoot(left), "home");
  assert.deepEqual(kinds(left), ["home", "album"], "Back leaves Preferences, the stack as it was");
  const viaLibrary = selectRoot(s, "library");
  assert.deepEqual(kinds(viaLibrary), ["albums"]);
  // Clicking the root Preferences was opened over: shown as it was, not popped.
  assert.deepEqual(kinds(selectRoot(s, "home")), ["home", "album"]);
});

test("a push never duplicates the target stack's top (also from Preferences)", () => {
  let s = selectRoot(start(), "library");
  s = push(s, album("a"));
  assert.equal(push(s, album("a")), s, "the album's own link: no change");
  s = selectRoot(s, "prefs");
  s = push(s, album("a"));
  assert.equal(activeRoot(s), "library");
  assert.deepEqual(kinds(s), ["albums", "album"], "Library shown, no second album entry");
  // Another album, or the same id on another source, is a new page.
  assert.deepEqual(kinds(push(s, album("b"))), ["albums", "album", "album"]);
  assert.deepEqual(kinds(push(s, album("a", "navidrome"))), ["albums", "album", "album"]);
});

test("replace swaps the top route only", () => {
  let s = selectRoot(start(), "library");
  s = replaceTop(s, { kind: "albums", source: "local", order: "year" });
  assert.deepEqual(kinds(s), ["albums"]);
  assert.equal(topEntry(s).route.order, "year");
});

test("a section change restarts Library's stack at that section and remembers it", () => {
  let s = selectRoot(start(), "library");
  s = push(s, album("a"));
  s = setSection(s, "artists");
  assert.equal(current(s).root, "library");
  assert.equal(current(s).section, "artists");
  assert.deepEqual(kinds(s), ["artists"]);
  assert.equal(sectionOf(topEntry(s).route), "artists");
  // Other roots keep theirs.
  assert.deepEqual(kinds(selectRoot(s, "home")), ["home"]);
  // The source's other stacks are not touched.
  assert.equal(s.sources.navidrome.section, "albums");
});

test("Folders is local only: on Navidrome it falls back to Albums", () => {
  assert.deepEqual(sectionsFor("navidrome").includes("folders"), false);
  assert.deepEqual(sectionsFor("local").includes("folders"), true);
  assert.equal(effectiveSection("folders", "navidrome"), "albums");
  const s = start({ local: "folders", navidrome: "folders" }, "navidrome");
  assert.deepEqual(kinds(selectRoot(s, "library")), ["albums"]);
  const local = selectRoot(setSource(s, "local"), "library");
  assert.deepEqual(kinds(local), ["folder"]);
  assert.equal(sectionOf(topEntry(local).route), "folders");
  const set = setSection(selectRoot(s, "library"), "folders");
  assert.equal(current(set).section, "albums");
  assert.deepEqual(kinds(set), ["albums"]);
});

test("Search: switches to its root with the query, not a push onto another root", () => {
  let s = selectRoot(start(), "library");
  s = push(s, album("a"));
  s = search(s, "yes");
  assert.equal(current(s).root, "search");
  assert.deepEqual(kinds(s), ["search"]);
  assert.equal(topEntry(s).route.query, "yes");
  assert.equal(topEntry(s).route.source, "local");
  // Library's stack stays where it was.
  assert.deepEqual(current(s).stacks.library.map((e) => e.route.kind), ["albums", "album"]);
  // A push from Search goes onto Search's stack; `/` (no query) returns
  // to the search page with the query it had.
  const page = topEntry(s);
  s = push(s, artist("x"));
  s = selectRoot(s, "home");
  s = search(s);
  assert.equal(current(s).root, "search");
  assert.equal(topEntry(s), page);
  assert.equal(search(s), s, "already there: no change");
});

test("source switch shows the other source's stacks; Preferences stays shown", () => {
  let s = selectRoot(start(), "library");
  s = push(s, album("a"));
  s = setSource(s, "navidrome");
  assert.equal(s.source, "navidrome");
  assert.equal(current(s).root, "home");
  s = push(s, album("n", "navidrome"));
  s = setSource(s, "local");
  assert.deepEqual(kinds(s), ["albums", "album"]);
  // From Preferences: Preferences on the other source too, the same page.
  s = selectRoot(s, "prefs");
  const page = topEntry(s);
  s = setSource(s, "navidrome");
  assert.equal(activeRoot(s), "prefs");
  assert.equal(topEntry(s), page);
  // Its browse roots are where they were.
  assert.deepEqual(kinds(back(s)), ["home", "album"]);
  assert.equal(setSource(s, "navidrome"), s);
});

test("Preferences does not leak into the other source after leaving it", () => {
  // Preferences, the switch there twice, Home, then Ctrl+2.
  let s = selectRoot(start(), "prefs");
  s = setSource(s, "navidrome");
  s = setSource(s, "local");
  s = selectRoot(s, "home");
  assert.equal(activeRoot(s), "home");
  s = setSource(s, "navidrome");
  assert.equal(activeRoot(s), "home", "Navidrome shows its own browse root, not Preferences");
  assert.deepEqual(kinds(s), ["home"]);
});

test("cross-source Go to opens on the root it was asked from (Preferences: Library)", () => {
  // From Home on Local, an album of Navidrome: Navidrome's Home stack.
  let s = selectRoot(start(), "home");
  let root = openRootFor(s);
  s = goTo(setSource(s, "navidrome"), root, album("n", "navidrome"));
  assert.equal(s.source, "navidrome");
  assert.equal(activeRoot(s), "home");
  assert.deepEqual(kinds(s), ["home", "album"]);
  // Navidrome was last on Library; the request from Local's Search lands on Search.
  s = selectRoot(setSource(s, "local"), "search");
  root = openRootFor(s);
  s = goTo(setSource(s, "navidrome"), root, artist("x", "navidrome"));
  assert.equal(activeRoot(s), "search");
  assert.deepEqual(kinds(s), ["search", "artist"]);
  // From Preferences: Library of the target source, Preferences left.
  s = selectRoot(setSource(s, "local"), "prefs");
  root = openRootFor(s);
  assert.equal(root, "library");
  s = goTo(setSource(s, "navidrome"), root, album("m", "navidrome"));
  assert.equal(activeRoot(s), "library");
  assert.deepEqual(kinds(s), ["albums", "album"]);
});

test("toRoot pops the shown stack to its root", () => {
  let s = start({ local: "folders", navidrome: "albums" });
  s = selectRoot(s, "library");
  s = push(s, { kind: "folder", path: ["a"] });
  s = push(s, { kind: "folder", path: ["a", "b"] });
  s = toRoot(s);
  assert.deepEqual(kinds(s), ["folder"]);
  assert.equal(toRoot(s), null);
  assert.equal(toRoot(selectRoot(s, "prefs")), null);
});

test("sourceReplaced resets that source's stacks to their roots only", () => {
  let s = start({ local: "genres", navidrome: "albums" });
  s = push(s, album("a"));
  s = selectRoot(s, "library");
  s = push(s, album("b"));
  s = setSource(s, "navidrome");
  s = push(s, album("n", "navidrome"));
  s = setSource(s, "local");
  s = resetSource(s, "local");
  assert.equal(activeRoot(s), "library", "the active root stays");
  assert.deepEqual(kinds(s), ["genres"]);
  assert.deepEqual(current(s).stacks.home.map((e) => e.route.kind), ["home"]);
  assert.deepEqual(
    s.sources.navidrome.stacks.home.map((e) => e.route.kind),
    ["home", "album"],
    "the other source is untouched",
  );
});

test("a section's Albums route names no sort: the remembered one applies", () => {
  assert.deepEqual(sectionRoute("albums", "local"), { kind: "albums", source: "local" });
  assert.deepEqual(sectionRoute("albums", "navidrome"), { kind: "albums", source: "navidrome" });
  assert.deepEqual(topEntry(setSection(selectRoot(start(), "library"), "albums")).route, {
    kind: "albums",
    source: "local",
  });
  // Without one in the route: the remembered sort, A–Z when none or unknown.
  assert.equal(localOrderOf(undefined, "mostPlayed"), "mostPlayed");
  assert.equal(localOrderOf(undefined, "year"), "year");
  assert.equal(localOrderOf(undefined, null), "name");
  assert.equal(localOrderOf(undefined, "frequent"), "name", "a server sort is no local order");
  assert.equal(serverSortOf(undefined, "frequent"), "frequent");
  assert.equal(serverSortOf(undefined, "mostPlayed"), "name");
  // A route's own sort (a choice just made) wins.
  assert.equal(localOrderOf("artist", "year"), "artist");
  assert.equal(serverSortOf("random", "newest"), "random");
});
