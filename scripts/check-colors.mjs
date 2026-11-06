// Fails when a colour value appears in `src/` (or `index.html`) outside
// the token layer (`src/styles/tokens.css`): components and stylesheets
// use `var(--…)` tokens only, so every theme can recolour all of the UI.
//
// Flagged everywhere: hex colours and colour functions (`rgb(`, `hsl(`,
// `oklch(`, `color(`, …). Named colours (all CSS named colours and the
// system colours such as `Canvas` or `AccentColor`): in CSS, in any
// declaration's value; in TypeScript and HTML, inside string literals
// (lowercase, as CSS is written, so UI labels like "Black" pass).
// Comments are ignored; `transparent`, `currentColor` and `inherit` are
// allowed.
//
// Run by `pnpm build` (and on its own: `pnpm check:colors`).

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const ALLOWED = new Set(["src/styles/tokens.css"]);
const EXTENSIONS = [".css", ".ts", ".tsx", ".html"];

// CSS Color 4 named colours.
const NAMED = `aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown
burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod
darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen
darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue
firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew
hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan
lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray
lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid
mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream
mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen
paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown
royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray slategrey snow
springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen`.split(/\s+/);

// CSS system colours (current and deprecated).
const SYSTEM = `AccentColor AccentColorText ActiveText ButtonBorder ButtonFace ButtonText Canvas CanvasText Field
FieldText GrayText Highlight HighlightText LinkText Mark MarkText SelectedItem SelectedItemText VisitedText ActiveBorder
ActiveCaption AppWorkspace Background ButtonHighlight ButtonShadow CaptionText InactiveBorder InactiveCaption
InactiveCaptionText InfoBackground InfoText Menu MenuText Scrollbar ThreeDDarkShadow ThreeDFace ThreeDHighlight
ThreeDLightShadow ThreeDShadow Window WindowFrame WindowText`.split(/\s+/);

// In code strings, only system colours that are not everyday words
// (`"menu"`, `"window"`, `"background"` are roles and property names).
const SYSTEM_IN_STRINGS = SYSTEM.filter((name) => /[a-z][A-Z]/.test(name));

const word = (names, flags) => new RegExp(`(?<![\\w-])(?:${names.join("|")})(?![\\w-])`, flags);
const HEX = /(?<![\w&])#[0-9a-fA-F]{3,8}\b/g;
const FUNCTION = /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/g;
// `background` is also a property name (`transition: background …`);
// the deprecated system colour of that name is not looked for.
const CSS_NAMED = word([...NAMED, ...SYSTEM.filter((name) => name !== "Background")], "gi");
const STRING_NAMED = word(NAMED, "g");
const STRING_SYSTEM = word(SYSTEM_IN_STRINGS, "g");
/** A declaration's value: after `prop:` up to `;`, `}` or the line's end. */
const DECLARATION = /(?:^|[;{\s])(--?[\w-]+|[a-z-]+)\s*:\s*([^;{}]*)/g;
/** Single-line string literals (template literals included). */
const STRING = /(["'`])((?:\\.|(?!\1)[^\\\n])*)\1/g;

/** The text with comments blanked out (line numbers kept). */
function stripComments(text, file) {
  const blank = (m) => m.replace(/[^\n]/g, " ");
  let out = text.replace(/\/\*[\s\S]*?\*\//g, blank);
  if (file.endsWith(".html")) out = out.replace(/<!--[\s\S]*?-->/g, blank);
  else if (!file.endsWith(".css")) {
    out = out.replace(/(^|[^:"'`\\])\/\/[^\n]*/g, (m, p) => p + blank(m.slice(p.length)));
  }
  return out;
}

/** Problems in one file's text: `[{ line, kind, text }]`. */
export function findColors(text, file) {
  const problems = [];
  const clean = stripComments(text, file);
  const lineAt = (index) => clean.slice(0, index).split("\n").length;
  const add = (kind, index, match) => problems.push({ line: lineAt(index), kind, text: match });
  for (const m of clean.matchAll(HEX)) add("hex colour", m.index, m[0]);
  for (const m of clean.matchAll(FUNCTION)) add("colour function", m.index, m[0]);
  if (file.endsWith(".css")) {
    // Over the whole text: a value may span lines. Selectors such as
    // `a:hover` parse as a "declaration" too, but their "values" are
    // pseudo-classes, never colour names.
    for (const d of clean.matchAll(DECLARATION)) {
      const start = d.index + d[0].length - d[2].length;
      for (const m of d[2].matchAll(CSS_NAMED)) add("named colour", start + m.index, m[0]);
    }
  } else {
    for (const s of clean.matchAll(STRING)) {
      const start = s.index + 1;
      for (const m of s[2].matchAll(STRING_NAMED)) add("named colour", start + m.index, m[0]);
      for (const m of s[2].matchAll(STRING_SYSTEM)) add("system colour", start + m.index, m[0]);
    }
  }
  return problems.sort((a, b) => a.line - b.line);
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
  for (const path of [join(root, "index.html"), ...files(join(root, "src"))]) {
    const rel = relative(root, path).split(sep).join("/");
    if (ALLOWED.has(rel)) continue;
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
