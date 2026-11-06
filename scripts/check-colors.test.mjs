// `node --test scripts/`: the colour check finds what it should.

import assert from "node:assert/strict";
import { test } from "node:test";
import { findColors } from "./check-colors.mjs";

const found = (text, file = "x.css") => findColors(text, file).map((p) => p.text);

test("flags colour values in stylesheets", () => {
  assert.deepEqual(found(".a { color: #fff; background: rgba(0, 0, 0, 0.5); }"), ["#fff", "rgba("]);
  assert.deepEqual(found(".a { border-color: white; fill: Black; }"), ["white", "Black"]);
  assert.deepEqual(found(".a { color: oklch(0.5 0.1 20); }"), ["oklch("]);
  assert.deepEqual(found("a { color: darkred; }"), ["darkred"]);
  assert.deepEqual(found("a { color: rebeccapurple }"), ["rebeccapurple"]);
  assert.deepEqual(found("a { color: Canvas; outline-color: AccentColor; }"), ["Canvas", "AccentColor"]);
  assert.deepEqual(found("a {\n  border: 1px solid\n    tomato;\n}"), ["tomato"]);
  assert.deepEqual(findColors("a {\n  border: 1px solid\n    tomato;\n}", "x.css")[0].line, 3);
  assert.deepEqual(found("  --x: navy;"), ["navy"]);
});

test("allows tokens, keywords and look-alikes", () => {
  assert.deepEqual(
    found(".a { white-space: nowrap; color: var(--text); border: 1px solid transparent; fill: currentColor; }"),
    [],
  );
  assert.deepEqual(found("/* #fff and white in a comment */ .a { color: var(--accent); }"), []);
  assert.deepEqual(found(".a { color: color-mix(in srgb, var(--accent) 50%, transparent); }"), []);
  assert.deepEqual(found(".tan, .menu:hover, mark { background: var(--surface-1); }"), []);
  assert.deepEqual(found(".a { --field-bg: var(--surface-1); }"), []);
  assert.deepEqual(found(".a { transition: background var(--motion-fast); }"), []);
});

test("flags colours in TypeScript strings, not labels or identifiers", () => {
  assert.deepEqual(found('const s = { background: "#123456" };', "x.tsx"), ["#123456"]);
  assert.deepEqual(found('el.style.color = "red";', "x.ts"), ["red"]);
  assert.deepEqual(found('const BAR = "white";', "x.ts"), ["white"]);
  assert.deepEqual(found("const b = `1px solid ${w} darkred`;", "x.ts"), ["darkred"]);
  assert.deepEqual(found('ctx.fillStyle = "CanvasText";', "x.ts"), ["CanvasText"]);
  assert.deepEqual(found('const label = "Black";', "x.ts"), []);
  assert.deepEqual(found('const role = "menu"; const w = window;', "x.ts"), []);
  assert.deepEqual(found("const red = 1; // white", "x.ts"), []);
  assert.deepEqual(found('const url = "http://x/y"; // #fff', "x.ts"), []);
  assert.deepEqual(found("const id = `${a}-${b}`; // hsl(1 2 3)", "x.ts"), []);
});

test("checks index.html", () => {
  assert.deepEqual(found('<body style="background: black">', "index.html"), ["black"]);
  assert.deepEqual(found('<meta name="theme-color" content="#000">', "index.html"), ["#000"]);
  assert.deepEqual(found('<!-- white --><div id="root"></div>', "index.html"), []);
});
