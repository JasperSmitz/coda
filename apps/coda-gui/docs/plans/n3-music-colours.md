# Brief N3 — Colours from the music (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Colour" (music colours, who takes which colours, blending, contrast). Builds on N2 (fixed tokens, the house accent, `prefs.accent`).

## Goal
Every cover yields a small palette; the player, the queue and Now Playing take the playing cover's colours, a page takes the colours of what it shows (with a glow behind its top), and colour changes blend slowly. "From the music" becomes the default accent mode.

## Scope

### 1. Palette (coda-core)
- New pure module `coda_core::cover::palette` (no I/O, no frontend types): `pub fn palette(image: &CoverImage) -> Palette` with `Palette { tint: Rgb, accent: Rgb, accent_text: Rgb, vivid: bool }` (`vivid == false` for near-greyscale covers).
  - Work on a small downsample (~32×32, `CoverImage::resized`). In OKLab/OKLCH: weight pixels by chroma (and lightly by mid lightness); find the **dominant hue** (hue histogram, ~24 bins, smoothed) and the **most vivid colour** (high chroma, mid lightness, from a populated hue, so one stray pixel does not win).
  - **Tint**: the dominant hue at OKLCH L ≈ 0.20, C ≤ ~0.05 (a deep dark; greyscale covers → C ≈ 0.01 with the cover's slight hue or the base's violet).
  - **Accent**: the vivid colour with L clamped to ~0.72–0.84 and C to ~0.08–0.17; then nudged up in L until contrast ≥ 3:1 on the base (`#110f14`, N2's `--bg` and the GUI's `palette::BASE`; core takes the base as a parameter or its own constant with a test/comment keeping them in step) **and** on its own tint. Greyscale covers: `vivid = false`, accent = the house accent (the frontend decides; core returns a soft neutral and the flag).
  - **Accent text**: black or white, whichever contrasts more with the accent.
  - Deterministic; colour maths in core (OKLab conversion, contrast). N2 left the colour maths in the GUI (`src-tauri/src/palette/color.rs`: hex parsing, contrast, OKLab/OKLCH, the lightness nudge; `src-tauri/src/palette.rs`: `BASE`, `SWATCHES`, accent text): move what both need into core (e.g. `coda_core::cover::palette` + a `color` submodule) instead of duplicating; the GUI keeps only what is GUI-specific (swatches, prefs validation) and calls core.
- Tests (core): solid colours (red, blue, near-black, white), a greyscale gradient (`vivid == false`), a mostly-dark cover with a small vivid area (accent from the populated hue, not a stray pixel), a two-hue cover (tint from the dominant), and the contrast guarantees for every case; same input → same output.

### 2. GUI backend
- Command `cover_palettes{urls: string[]}` → `{[url]: PaletteDto | null}` (`PaletteDto { tint, accent, accentText, glow, vivid }`, hex strings; `glow` is the accent at ~45% alpha, `#rrggbbaa`). URLs are the cover URLs the webview already has (`coda://…/cover/<token>`), resolved through the existing registry (`key_for_url`; unknown → `null`). Loads the small cover through the existing loader/disk cache (respect its semaphore), computes on a blocking thread, caches palettes in a bounded LRU keyed by `CoverKey` (≈4,000). Never blocks the engine actor. At most ~64 urls per call. Capability added to the minimal list.
- Tests: unknown token → null; cache hit does not reload; bounded size; batch limit.

### 3. Frontend
- `lib/palette.ts` + a small store/hook (`usePalette(url)`, `usePalettes(urls)`): batches requests within a frame, caches in memory, returns `null` until known.
- **Scoping** (`state` + one helper that writes `--accent`, `--accent-text`, `--accent-soft`, `--glow`, `--tint`, `--viz` on an element): 
  - The **playing cover** → the bar (`.bar`, still the 1.x docked bar until N4), the queue panel, Now Playing (its tint behind the backdrop, its accent for the timeline, play button, visualizer).
  - The **page** (`.main`/the view container) → the subject of the top route: album page its cover; artist page its image (or first album); playlist its cover (or first track's); genre page its first album's; Home: nothing yet (N6). Other routes: unscoped (house accent).
  - With a fixed accent (`prefs.accent` a hex), scopes still set `--tint` and the glow image but not the accent tokens.
  - A greyscale cover (`vivid == false`): accent tokens stay the house accent; the tint applies.
- **Blending**: register `--accent`, `--accent-soft`, `--glow`, `--tint`, `--viz` with `@property` (`syntax: "<color>"`) in `tokens.css` and transition them (accent/glow ~1.6s, tint ~2s, ease) on the scoped containers. Reduced motion: no transition. Where `@property` is unsupported the values switch at once (no JS fallback needed).
- **Page glow** (`components/PageGlow.tsx`): behind the top of the page content (under the header), the subject's cover (`/large` or `/small`) very blurred (~100px, saturate ~1.4, ~60% opacity) under a radial darkening, masked to fade out by ~600px; two layers crossfading (~1.8s) when the subject changes; no glow on unscoped routes; `pointer-events: none`; does not repaint on scroll (its own layer). Until N5 the views have a fixed header with a scrolling body: place the glow so it sits behind the header and the top of the body and stays put; N5 makes it scroll with the page.
- **The docked bar**: add the playing cover, enlarged and heavily blurred, as its background under a `--tint` wash (crossfading on track change), so the N4 player starts from it.
- **Preferences → Accent**: "From the music" first (default) with a short explanation ("A page takes the colours of the album it shows, the player those of the song that's playing."), then the swatches and custom colour. `prefs.accent` gains `"music"`; a missing key reads as `"music"`; an existing `"house"` from N2 stays house.

## Out of scope
Per-tile accents on grid cards (N5, using the batch command), Home's carousel glow (N6), the floating player (N4).

## Acceptance
- Core palette tests as above pass; palettes are deterministic; the contrast guarantees hold for every test image (assert them).
- Opening an album page tints its top and its buttons within ~2s, smoothly; going back crossfades away; the bar, queue and Now Playing follow track changes; nothing flashes; a fixed accent overrides accents everywhere; greyscale covers fall back to the house accent.
- No render or engine stall: palettes load off the actor; a page renders before its palette arrives (no layout change when it does).
- Checks (report commands): `pnpm build`, `pnpm check:colors` (from `apps/coda-gui`); `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets` (core changed: the TUI must still build and pass).
- Docs: `docs/ARCHITECTURE.md` (GUI: command, store, scoping; root `docs/ARCHITECTURE.md`: the core palette module), `docs/DESIGN.md` (mark the music parts of Colour *(shipped)*), `scripts/smoke.md` ("2.0 N3 — Colours" section).
- Do not commit; report changed files, decisions, checks, and what is open.
