# Brief R2: Refinement pass 2, the "Washed" page colour (2.0)

The owner chose from the lab mockup (2026-11-20). R1 is committed (8ce67ed); this builds on it.

The owner's choices:
- **Page colour: Washed.** The whole page takes its subject's colour, never fading to grey-black. "Bold is a bit much and glow is far too little."
- **Ink: Always light.** Text stays light; the background is kept dark enough for it.
- **Containers: Floating.** Thin dividers were "nice", but between track rows they look "mathematics-y". The cards did not look good over the floating layout. So keep the free-floating layout and add only quiet section dividers (§3).
- **Speed: Medium.** This is what R1 already set: colour 0.6–0.7s, carousel 0.5s. No change.
- **Swipe** (from R1's smoke run): it sometimes does not register, especially on Search.

Reference: the lab mockup, `neo-coda-lab.html` in the session scratchpad, with "Washed" + "Always light". The architect has it, and the developer of R1 has seen its description.

## Scope

### 1. The page ground takes its subject's colour (core + GUI)
- **The ground colour.** Core's palette gains a `ground` colour for each cover: the dominant hue at a deep but clearly coloured level (roughly OKLCH L 0.26–0.34, with chroma as high as the sRGB gamut allows, capped). It is as light as possible while text ≥7:1, text-2 ≥4.5:1 and text-3 ≥3:1 still hold on it, using the solid text levels from `tokens.css` (keep the tokens test in step).
  - A greyscale cover gets a near-neutral, slightly warm or cool ground, never a flat grey (as in the lab).
  - Test it over many hues, plus white, black and yellow covers, and check the contrast floors.
- **Where it applies.**
  - Every page with a subject (`usePageSubject`) is coloured, edge to edge and the whole page down: the Home carousel's focus, album, artist, playlist, mix, genre and folder pages where they report a cover. The glow (R1) sits on top and fades into the ground, not into the base. Its `::after` shade and mask end in the ground colour.
  - Pages without a subject (Search, Library roots, Preferences) keep the base. The owner said earlier that the window must not follow the playing song.
  - Home's Moods section sits on the same ground as the carousel above it (the lab's look), not on the base.
- **The ground behind and around the page.** Whatever paints behind or around the page uses the ground:
  - the top bar's fade-out gradient (`.topbar::before`, which uses `--bg` today);
  - the window or `.main` background;
  - anything that uses `--bg` as "the page's colour" inside the page: the mood card ground, sticky tool-row glass, letter index, skeletons.

  Add a token for this, e.g. `--ground`, which is the base when there is no subject, and use it everywhere `--bg` means "the page's colour". Keep `--bg` where it means the fixed base: the native window colour, Now Playing, and contrast calculations that assume it.
- **Glass on a coloured page.** The floating glass over the page should not read as a grey slab on a coloured page: the top bar's pill, the Preferences button, sticky tool rows, and menus opened over the page. Tint it with the ground, e.g. `--glass` as a dark mix of `--ground` at the same opacity. The text-2 ≥4.5:1 guarantee over the worst case stays: re-check it at the worst point (LESSONS).
- The player, the queue and Now Playing still follow the playing track (unchanged).

### 2. Blends without `@property`
- The ground changes with the subject (carousel moves, navigating), blending over `--motion-tint` (~0.7s).
- Do it with standard properties (`background-color`, `color`, `border-color` and the like, set from `var(--ground)` and transitioned) or with crossfaded layers. Do not rely on registered custom property interpolation, which may not run in WebKit.
- Page transitions (N7) still crossfade the content. The ground behind them blends and does not flash the base between pages. Going from a coloured page to a page without a subject blends to the base.
- Reduced motion: instant.

### 3. Quiet section dividers (Floating stays)
- A single soft hairline (`--border`, or a softer token) between the large sections of a page:
  - Home: between "Where you left off" and "Moods for today";
  - artist pages: between the header, "Your most played" and the discography;
  - album pages: between the header and the track list, and before "More by …" if present.
- **No dividers between track rows or list rows.**
- Inset to the content width, not edge to edge.
- No new cards or panels.

### 4. Swipe responsiveness
- After R1 the swipe sometimes does not register, especially on Search. Find the cause. Candidates:
  - **The "can still scroll sideways" check.** A page scroller (or an ancestor) whose `scrollWidth` exceeds its `clientWidth` by a pixel or two while its `overflow-x` is hidden or clipped counts as scrollable, so the swipe is swallowed. Only count elements whose computed `overflow-x` is `auto` or `scroll`, and allow a ≥2px margin.
  - **The search field.** It is ignored when the wheel event starts over an input; on Search the large field is at the top. Allow swipes over a text input that cannot scroll sideways itself, i.e. its content is not wider than the field.
  - **Locks that are too long.** The 600ms cooldown, plus the 250ms gap, plus "no gesture can start while a page change runs" can swallow a deliberate second swipe.
- Change the lock to end as soon as a **new gesture** is detected, not only on a pause. Momentum events decay monotonically, so a rise in |deltaX| after a decay (by a clear factor, over a few events) means a new swipe has started.
  - Shorten the cooldown to ~350ms.
  - A gesture may start once the page change has finished.
- Keep the guarantee that one firm swipe with a long momentum tail switches exactly once.
- Tests:
  - a second deliberate swipe 400ms after the first one switches again;
  - the momentum-tail case still switches once;
  - a swipe over the Search field switches;
  - a page scroller with `overflow-x: hidden` does not block the swipe.

## Out of scope
Fonts, the record flourish, adaptive dark ink, Bold, cards or shelves, Now Playing.

## Acceptance
- Every page with a subject is coloured top to bottom with its ground. Text contrast holds: tested in core from the CSS constants, and with the glass re-checked at its worst point.
- Pages without a subject stay on the base.
- Moving the carousel and navigating blends the ground with no flash of the base.
- The swipe tests above pass.
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (and `--all-features`), `cargo test --workspace --all-targets`.
- Docs:
  - `docs/DESIGN.md`: Colour (the ground, glass on colour), Home, Library pages, the dividers, the swipe.
  - `docs/ARCHITECTURE.md`, and the root `docs/ARCHITECTURE.md` for the core palette.
  - `scripts/smoke.md`: a "2.0 R2" section.
  - `docs/PLAN.md`: R2 under "Refinement passes".
- Do not commit; report changed files, decisions, checks, open items. This pnpm does not accept `-s`. No GUI windows.
