// `node --test scripts/`: the ground field's look (src/lib/groundField.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { fieldBackground, fieldKey } from "../src/lib/groundField.ts";

test("no stops: the base", () => {
  assert.equal(fieldKey(null), "base");
  assert.equal(fieldKey([]), "base");
  assert.deepEqual(fieldBackground("base"), { background: "var(--bg)" });
});

test("one stop fills; more rise from the bottom corners, the first on top of the page", () => {
  assert.deepEqual(fieldBackground(fieldKey(["#112233"])), { background: "#112233" });
  const three = fieldBackground(fieldKey(["#111111", "#222222", "#333333"])).background;
  assert.equal(
    three,
    "radial-gradient(95% 80% at 100% 100%, #222222, #22222200), " +
      "radial-gradient(95% 80% at 0% 100%, #333333, #33333300), #111111",
  );
  // A new set of stops is a new field (it crossfades).
  assert.notEqual(fieldKey(["#111111", "#222222"]), fieldKey(["#111111", "#333333"]));
});
