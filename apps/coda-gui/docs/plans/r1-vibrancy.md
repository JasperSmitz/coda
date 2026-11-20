# Brief R1 — Refinement pass 1: vibrancy, smooth colour, Home heading, top bar, swipes (2.0)

From the owner's first look at the running 2.0 build (2026-11-20). Approved: "let's see if you can do anything with that". Reference: the v3 mockup (lively, vibrant), DESIGN.md "Colour", "Home", "Top bar", "Player".

## What the owner saw, and why

1. **"A lot less vibrant, very dark and muddy" compared to the mockup.** Causes found in the code:
   - The page glow (`app.css` `.page-glow`) shows the cover at opacity 0.45 under a wash of **65–100% of the tint**. The mockup showed it at 0.62 with `saturate(1.4)` under a radial shade that is **transparent in the middle** and only darkens towards the edges. The wash was sized for the worst case, an all-white cover, so every cover pays for it.
   - The player's backdrop (`.player-backdrop::after`) is **86% tint**; the mockup used 50% with `saturate(1.7)`. Same worst-case sizing.
   - Mood cards (`browse.css` `.mood-card::before`) are a gradient of three **tints**. Tints are dark by design (L≈0.20), so the cards are mud. The mockup used vivid hues at ~45% lightness and 35–50% alpha over glass.
2. **The player "takes a tiny centre sliver" of the cover.** `.player-backdrop .glow-layer` is 120% wide × 500% tall with `object-fit: cover`. A square cover becomes ~1200×1200, and only the middle ~30% band of it shows. For Muse's *The Resistance* (a colourful ring around a dark orange road), only the road shows.
3. **Colour "snaps rather than transitions once the UI settles" when the carousel moves.** The likely cause: the carousel's covers are already in the image cache, so a new glow layer's `onLoad` fires straight away, and `is-shown` is committed before the browser has drawn the layer at opacity 0. Then no transition runs and it snaps (the comment in `PageGlow.tsx` already notes this for `transitionend`). Also, the washes use `--tint` through registered-property transitions (`@property`), which may not interpolate in every WebKit. Don't rely on them for large surfaces.
4. **Home heading centred, Moods heading left-aligned underneath:** the owner prefers left-aligned, offset like the mockup.
5. **The top bar's Home / Search / Library pill is small and too close to the top.** In macOS full screen, pointing at it reveals the system menu bar.
6. **Horizontal swipe:** the owner wants a two-finger horizontal swipe to switch **Home ↔ Search ↔ Library**, and not the carousel. On the carousel a small swipe skips an album; the sensitivity is off.

## Scope

### 1. Contrast per cover, not for the worst case (core)
- In `coda_core::cover::palette`, add what the frontends need to wash a cover only as much as **that cover** needs:
  - a measure of how bright the cover gets once blurred (e.g. the maximum relative luminance over a coarse grid of cell averages, roughly 6×6, which approximates a heavy blur);
  - for each surface that draws text over a blurred cover, the **minimum wash alpha** (of the tint, sRGB compositing as the webview blends) so that text-2 (`#adaab1`) keeps ≥4.5:1 and the text keeps ≥7:1 over the brightest cell.

  The surfaces are the player and the page glow. Each has its own constants: the opacity and the filter brightness the CSS draws the cover with.
  - Put the maths in core with tests (white cover, black cover, bright yellow, a dark vivid cover needing little or no wash).
  - Keep the CSS constants in step with a test, as `palette::BASE` is kept now.
  - Give the wash a floor, so it never goes fully transparent (e.g. ≥0.25 for the page glow, ≥0.35 for the player).
- Add **swatches**: up to 4 distinct vivid colours of the cover, by area, with distinct hues, at a lightness and chroma that read as colour over the base (roughly OKLCH L 0.55–0.70, C as high as the gamut allows). Mood cards, genre tiles and other gradients use these. A greyscale cover has no swatches.
- Extend the DTO (`Palette` in `src-tauri` and `lib/types.ts`) and the palette cache's format. Bump its version, so old cached palettes are recomputed rather than read without the new fields.

### 2. The page glow, like the mockup
- The cover at ~0.6–0.65 opacity with `blur(~110px) saturate(~1.5)`, and a radial shade that is light or clear in the middle and goes to the base at the edges and the bottom (the mockup's `.glow::after`). The wash amount comes from that cover's computed alpha (§1).
- The glow still fades out down the page and still scrolls away with the header.

### 3. The player shows the whole cover
- Draw the cover **stretched across the whole strip** (`object-fit: fill`, or equivalent, covering the strip with a small bleed for the blur): the left of the cover is at the left of the strip, the right at the right. Every region's colour shows. Use blur ~28–32px and `saturate(~1.7)`, under the per-cover wash (§1).
- Keep the queue panel's tinted glass in step if it uses the same backdrop.
- Now Playing is **not** changed (the owner loves it).

### 4. Colour that blends, always
- A glow layer (page and player) must always fade in, even when its image comes from the cache. Use a CSS **animation** for the fade-in, which needs no "before" style, or otherwise guarantee that the hidden state is drawn first. The fade-out of older layers stays a transition, or becomes an animation too.
- **Each layer carries its own wash** (that cover's tint and wash alpha), so the whole backdrop (image and wash together) crossfades as one. It must not depend on `@property` interpolation of `--tint` for these large surfaces.
- Small things may keep the registered-property blends (the accent on buttons, the visualizer).
- A layer waits for both its image and its palette before it fades in.
- Moving the carousel quickly gives a continuous blend: a layer that is mid-fade continues from where it is, and nothing jumps. Keep at most 3 layers.
- Check the carousel-to-glow path for any wait that holds the colour back until the carousel "settles" (the owner's words). The glow should start blending as soon as the focus changes and the next cover is ready.
- Reduced motion: instant, as now.

### 5. Mood cards and genre tiles in colour
- Mood cards: radial gradients of the first album's swatches over glass, like the mockup (`radial-gradient(120% 90% at 15% 0%, <swatch1> ~50% alpha, transparent 65%), radial-gradient(90% 80% at 100% 100%, <swatch2> ~35%, transparent 70%), var(--glass)`), or a richer mix of the three albums' swatches.
- Genre tiles (N7) likewise.
- Text on them keeps the DESIGN.md contrast rules (text-2 ≥4.5:1), checked at the worst point of the gradient (LESSONS: contrast at the worst point of a layered background).
- Without swatches (greyscale, or not known yet): the current look.

### 6. Home heading
- "Where you left off" and its eyebrow are left-aligned, starting at the same left edge as "Moods for today", with the ‹ › buttons at the right end of the heading row (as in the mockup's `.home-head`).
- The meta block under the carousel (title, artist, the quiet line, the buttons) stays centred under the focused cover.

### 7. A bigger, lower navigation pill
- Taller tabs (~40px; about 15px labels; more horizontal padding), with the pill's top at least ~14px below the window's top edge. A pointer on it must not reach the strip where macOS full screen reveals the menu bar.
- Grow `--topbar-h` as needed (~64–68px), and update everything that depends on it: page clearance, sticky tool rows, scroll-margins.
- On macOS, move the window buttons so that their centre lines up with the pill's centre (`titlebar.rs` `BUTTONS`, `--window-buttons-w` if affected); the owner confirms in the smoke check.
- The brand, Preferences button and indicator are resized to match.

### 8. Swipe between Home, Search and Library
- A two-finger horizontal swipe (wheel events with |deltaX| > |deltaY|) on the page switches the root, one step per gesture: left swipe → next (Home → Search → Library), right swipe → previous. At the ends, nothing happens.
  - A gesture fires once its accumulated deltaX passes a threshold (~120px).
  - It then stays locked until the wheel events stop (≥ ~250ms without any, which also swallows momentum events). One gesture never switches twice.
- Ignored when the wheel event starts inside:
  - a horizontally scrollable element that can still scroll that way;
  - a text field or slider;
  - the queue panel, Now Playing, a dialog or a menu.
- Ignored while a page transition is running.
- It goes through the normal navigation (root switch, page transition, scroll restore), as clicking the tab does. Within Library it switches roots, not sections.
- The carousel no longer reacts to horizontal swipes. Remove its wheel handler. Arrows, ‹ ›, clicks and keys stay.

## Out of scope
Fonts, the record flourish, Now Playing, new features.

## Acceptance
- Over a dark vivid cover the page glow and player are clearly colourful (the wash near its floor). Over an all-white cover, text-2 still has ≥4.5:1 (tested in core from the CSS constants).
- The player's backdrop shows colours from the whole cover.
- A glow crossfade over a cached cover blends over its full duration. Rapid carousel moves never jump.
- The swipe switches exactly one root per gesture, never inside the excluded elements, and the carousel ignores swipes.
- Checks (report commands): `pnpm build`, `pnpm check:colors`, `node --test` (as the scripts run it); `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --all-targets` (core changes).
- Docs:
  - `docs/DESIGN.md`: Colour (washes per cover, swatches), Home, Top bar, the swipe.
  - `docs/ARCHITECTURE.md`.
  - `README.md`: the swipe.
  - `scripts/smoke.md`: a "2.0 R1" section covering vibrancy over dark and bright covers, The Resistance in the player, the carousel blend, the pill and window buttons in full screen, and the swipe incl. the excluded places.
  - `docs/PLAN.md`: a "Refinement passes" entry.
- Do not commit; report changed files, decisions, checks, open items. This pnpm does not accept `-s`.
