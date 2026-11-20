# Brief R3: Refinement pass 3, colour that represents the whole cover, and a swipe that never skips (2.0)

From the owner's run of R2 (7b642a8), 2026-11-20. Approved: "I think this wash style mixed with a glow from the literal art itself might work well."

## What the owner saw
1. **"The swiping still gets a bit overzealous and tends to skip."** This is after three rounds of heuristics: a lock, a cooldown, then rise detection by speed. We don't have a single trace of what the webview actually delivers on the owner's MacBook.
2. **The ground is one colour, often not representative.** *The Resistance* (a ring of many hues around a dark orange road) becomes one shade of red. Many albums become a single colour that doesn't stand for the art as a whole. The owner likes the washed look, but wants the cover's own colours to "expand out" behind the album, at least in the carousel area.

## Scope

### 1. Swipe: simpler rules and real data
- **A trace (dev builds only).** When `import.meta.env.DEV` is set, record every wheel event the swipe sees:
  - its time stamp, deltaX, deltaY and deltaMode;
  - whether a page change is running;
  - the phase or decision (start, accumulate, switch, locked, ignored and why).

  Write these lines to the app's log file through a debug-only path (an existing frontend log command if there is one, otherwise a small command compiled only with `debug_assertions`). Batch the writes, with no I/O per event on the engine. Redact nothing sensitive: there is nothing sensitive in it. Document in `smoke.md` where the owner finds the log, so they can reproduce a skip and the architect can read it.
- **Simpler rule.** Remove the rise detection. One switch per continuous stream of wheel events:
  - A stream ends only after a quiet gap of ≥ `SWIPE_GAP_MS` (start at 220ms) with no wheel events at all, measured on the event time stamps.
  - After a switch, the stream is locked until it ends. Also, nothing may start until `SWIPE_SETTLE_MS` (start at 300ms) after the page change has finished. That absorbs events the webview held back while drawing the new page and delivered late, which is the likely cause of the skips.
  - A new stream that starts before that point is ignored for its whole length, not remembered.
  - Keep the threshold of 120px travel, the direction fixed by the first event, and the R2 exclusions (`blocksSwipe`).
- Constants live in one place, so they can be tuned from the trace.
- Tests:
  - a long momentum tail, also in batches, switches once;
  - events delivered late after a page change (a 300ms gap, then a burst) do not switch;
  - a second swipe after the quiet gap and the settle time switches;
  - the exclusions still hold.
- Update the swipe's LESSONS draft only once the owner's trace shows the cause. Not in this slice.

### 2. Colour that represents the whole cover

**2a. The ground is a blend of the cover's colours, not one hue.**
- Core: rank the swatches by visual weight (area × chroma, or similar) rather than area alone, so a large dark region does not outvote a colourful ring.
- For each of the up-to-3 heaviest swatches, compute a **ground stop**: that hue at the same text-safe depth rules as today's ground (text ≥7, text-2 ≥4.5, text-3 ≥3, also under the `--selected` veil).
- A greyscale cover keeps one neutral ground. A cover with one colour gets one stop.
- The page's ground becomes a soft multi-stop field, e.g. the base stop as a fill with radial gradients of the second and third stops from different corners. Every point of the field mixes text-safe stops, so contrast holds everywhere (test the worst mix, at least the pairwise midpoints).
- Mixing in sRGB can produce mid-points darker or muddier than either stop. If OKLab mixing is needed for clean mid-points, precompute 1–2 intermediate stops in core instead of relying on `color-mix` in OKLab support.
- `--ground` (the single colour used by glass tints and the top bar's fade) becomes the first stop.
- Blending: gradients don't transition, so the ground field crossfades as layers (as the glow layers do): opacity animations, Web Animations or CSS, from the current opacity, with no `@property`. Keep the R2 rules: no dip to the base between coloured pages, and the pending subject held for up to 1.5s.

**2b. A bloom of the actual art behind the cover (the carousel, and album and playlist headers).**
- Behind the focused carousel cover (and behind a page header's cover):
  - the cover image itself, scaled to about 2–2.6× the cover, blurred (about 50–80px), `saturate(~1.6)`, at high opacity close to the cover;
  - masked radially so it fades out into the ground field well before the page edges and before the text that sits over it. On Home that means the heading at the top left and the title and buttons under the cover; on headers, the title block.
- The cover's own colours should be visible "expanding out" around it: the ring's hues of *The Resistance* around its cover, not one red.
- Contrast: text must never sit on the bright part. Either:
  - place the mask so that the heading and meta text are over the faded part, and check text-2 ≥4.5:1 at the worst point of the bloom's remaining opacity over the brightest cell (core measures it as for the page glow, with this surface's constants and its own test); or
  - lay a local ground scrim under the text block.

  Choose one, and say which.
- The bloom follows the carousel focus with the same crossfade (Medium speed: ~0.6s), and moves with the carousel. It must not lag behind the transforms or re-blur on every frame:
  - use one composited layer per bloom;
  - it may sit still while the covers slide, crossfading to the new focus.
- Reduced motion: an instant swap.
- **R1's full-width page glow at the top becomes redundant where a bloom exists.** Drop it on those pages, or keep it much quieter (decide and say which), so the page reads as the ground field plus a bloom, not three layers of colour.
- Performance: the bloom and the ground layers must stay smooth while the carousel moves and the page scrolls. A 2.6× blurred image at 330px is about 860px: check that it doesn't cause scroll jank (keep the `will-change` budget small).

## Out of scope
Fonts, the record flourish, the ground's base lightness (the owner hasn't asked), Now Playing, the player island (unchanged since R1).

## Acceptance
- A multi-colour cover gives a ground of 2–3 of its hues and a bloom showing its actual colours around the cover. A single-colour cover is unchanged in spirit. A greyscale cover stays neutral.
- Every text level keeps its contrast over every point of the ground field and over the faded bloom (tested in core from the CSS constants).
- Carousel moves blend the ground and bloom with no jank and no flash.
- The swipe tests pass, and the dev trace is written and documented.
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (and `--all-features`), `cargo test --workspace --all-targets`.
- Docs:
  - `docs/DESIGN.md`: Colour (the ground field, the bloom), Home, Library pages, the swipe.
  - `docs/ARCHITECTURE.md` and the root `docs/ARCHITECTURE.md`.
  - `scripts/smoke.md`: a "2.0 R3" section, including where the swipe trace is.
  - `docs/PLAN.md`.
- Do not commit; report changed files, decisions, checks, open items. This pnpm does not accept `-s`. No GUI windows.
