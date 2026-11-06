// Fails when a colour value appears in `src/` outside the token layer
// (`src/styles/tokens.css`): components and stylesheets use `var(--…)`
// tokens only, so every theme can recolour all of the UI. Comments are
// ignored; `transparent`, `currentColor` and `inherit` are allowed.
//
// Run by `pnpm build` (and on its own: `pnpm check:colors`).

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const src = join(root, "src");
const ALLOWED = new Set([join("src", "styles", "tokens.css")]);
const EXTENSIONS = [".css", ".ts", ".tsx"];

const NAMED = [
  "white", "black", "red", "green", "blue", "yellow", "orange", "purple", "pink", "brown",
  "gray", "grey", "silver", "maroon", "navy", "teal", "olive", "lime", "aqua", "fuchsia",
  "gold", "cyan", "magenta", "indigo", "violet",
];
const PATTERNS = [
  { name: "hex colour", re: /(?<![\w&])#[0-9a-fA-F]{3,8}\b/g },
  { name: "colour function", re: /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/g },
  // `white-space`, `--black-x` or `isRed` are not colours.
  { name: "named colour", re: new RegExp(`(?<![\\w-])(?:${NAMED.join("|")})(?![\\w-])`, "gi") },
];

/** The text with comments blanked out (line numbers kept). */
function stripComments(text, file) {
  const blank = (m) => m.replace(/[^\n]/g, " ");
  let out = text.replace(/\/\*[\s\S]*?\*\//g, blank);
  if (!file.endsWith(".css")) out = out.replace(/(^|[^:"'`\\])\/\/[^\n]*/g, (m, p) => p + blank(m.slice(p.length)));
  return out;
}

/** Problems in one file's text: `[{ line, kind, text }]`. */
export function findColors(text, file) {
  const problems = [];
  const lines = stripComments(text, file).split("\n");
  lines.forEach((line, i) => {
    for (const { name, re } of PATTERNS) {
      for (const match of line.matchAll(re)) {
        // Strings shown to the user may say "black" in prose: only flag
        // named colours in CSS and in style-ish TS code.
        if (name === "named colour" && !file.endsWith(".css") && !/style|color|background|fill|stroke/i.test(line)) {
          continue;
        }
        problems.push({ line: i + 1, kind: name, text: match[0] });
      }
    }
  });
  return problems;
}

function* files(dir) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) yield* files(path);
    else if (EXTENSIONS.some((ext) => entry.endsWith(ext))) yield path;
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  let count = 0;
  for (const path of files(src)) {
    const rel = relative(root, path);
    if (ALLOWED.has(rel) || ALLOWED.has(rel.split(sep).join("/"))) continue;
    for (const p of findColors(readFileSync(path, "utf8"), path)) {
      console.error(`${rel}:${p.line}: ${p.kind} \`${p.text}\` outside the token layer (use a var(--…) token)`);
      count++;
    }
  }
  if (count > 0) {
    console.error(`\n${count} colour value(s) outside src/styles/tokens.css.`);
    process.exit(1);
  }
  console.log("check-colors: no colour values outside the token layer");
}
