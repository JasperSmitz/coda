// The page's ground field's look (components/GroundField), pure
// (`node --test scripts/`, scripts/groundField.test.mjs): a key per field
// (its stops) and its background: the first stop as the fill, the second
// rising from the bottom right, the third from the bottom left, so the top
// (under the top bar and the header) is the first stop, `--ground`.

/** A layer's inline style. */
export interface FieldStyle {
  background: string;
}

/** The key of a field: its stops (`base`: none). */
export function fieldKey(stops: readonly string[] | null): string {
  return stops && stops.length > 0 ? stops.join(" ") : "base";
}

/** A field's background: the first stop as the fill, the others from the bottom corners. */
export function fieldBackground(key: string): FieldStyle {
  if (key === "base") return { background: "var(--bg)" };
  const [fill, second, third] = key.split(" ");
  const layers: string[] = [];
  // Each fades to its own colour at alpha 0 (not `transparent`, black at
  // alpha 0), so no mid-point darkens wherever premultiplied
  // interpolation is not used.
  if (second) layers.push(`radial-gradient(95% 80% at 100% 100%, ${second}, ${second}00)`);
  if (third) layers.push(`radial-gradient(95% 80% at 0% 100%, ${third}, ${third}00)`);
  layers.push(fill);
  return { background: layers.join(", ") };
}

