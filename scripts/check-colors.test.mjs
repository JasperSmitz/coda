// `node --test scripts/`: the colour check finds what it should.

import assert from "node:assert/strict";
import { test } from "node:test";
import { findColors } from "./check-colors.mjs";

const kinds = (text, file = "x.css") => findColors(text, file).map((p) => p.text);

test("flags colour values in stylesheets", () => {
  assert.deepEqual(kinds(".a { color: #fff; background: rgba(0, 0, 0, 0.5); }"), ["#fff", "rgba("]);
  assert.deepEqual(kinds(".a { border-color: white; fill: Black; }"), ["white", "Black"]);
  assert.deepEqual(kinds(".a { color: oklch(0.5 0.1 20); }"), ["oklch("]);
});

test("allows tokens, keywords and look-alikes", () => {
  assert.deepEqual(
    kinds(".a { white-space: nowrap; color: var(--text); border: 1px solid transparent; fill: currentColor; }"),
    [],
  );
  assert.deepEqual(kinds("/* #fff and white in a comment */ .a { color: var(--accent); }"), []);
  assert.deepEqual(kinds(".a { color: color-mix(in srgb, var(--accent) 50%, transparent); }"), []);
});

test("checks TypeScript too, without flagging prose", () => {
  assert.deepEqual(kinds('const s = { background: "#123456" };', "x.tsx"), ["#123456"]);
  assert.deepEqual(kinds('el.style.color = "red";', "x.ts"), ["red"]);
  assert.deepEqual(kinds('const label = "Black";', "x.ts"), []);
  assert.deepEqual(kinds('const url = "http://x/y"; // #fff', "x.ts"), []);
  assert.deepEqual(kinds("const id = `${a}-${b}`; // hsl(1 2 3)", "x.ts"), []);
});
