# Brief N7 — Motion and polish (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Motion" ("New in 2.0"), "Shape and surfaces". Builds on N1–N6. The owner-taste items of N7 (choosing the final typefaces, the record flourish, the nitpick round, the 2.0.0 release) are **not** in this brief; they happen in refinement passes with the owner.

## Scope

### 1. Page transitions
- Navigating between pages (push, Back, root switches, section changes) runs as a View Transition through `lib/motion.ts` `viewTransition` (no new mechanism): the page content crossfades (~0.35s); the top bar, the player and an open queue panel do not move or fade (give them their own `view-transition-name`s, or exclude them, so they are not part of the root crossfade, and make sure Now Playing's names and the N4 `np-morph` never collide).
- **Cover morphs**: opening an album, playlist or artist from a cover (grid tile, list row cover, Home carousel's focused cover, a mood card's cover, search's top result) morphs that cover into the page header's cover (the stable header cover from N5a); Back morphs it back into the same tile when that tile is rendered (else a plain crossfade). One name at a time, set just for the transition and cleared after (also when interrupted).
- Scroll restore on Back must land before the new snapshot is taken (no visible jump after the transition).
- Without View Transitions (WebKitGTK versions lacking them) and with reduced motion: no animation, instant navigation.
- Rapid navigation (holding Backspace, quick clicks) never queues transitions: an interrupted one jumps to its end (as the existing helper does).

### 2. Small motion
- The N1 navigation indicator, the N4 queue rise, the N6 carousel and the N3 colour blends exist; check they share the curves and durations of DESIGN.md "Motion" (`--ease`, `--spring` only for small direct responses) and fix strays.
- Cover tiles: hover lift (transform only, `--spring`), already partly there from N5b; the round Play button scales in. Pressed buttons scale slightly (N2). No other new effects.

### 3. Onboarding restyle
- The first-run flow on the 2.0 look: a glass card over the base with a soft glow, the coda sign and wordmark, display type for its headings, pill buttons; its steps and behaviour unchanged.

### 4. Genres and Playlists in the 2.0 style (DESIGN.md "Library")
- **Genres**: tiles in a responsive grid, each tinted with the genre's best-known cover's colours (the N3 palette of its first album's cover; house colours until known), the name in display type, the album count; a tilted cover peeking from the corner. Keyboard and the existing genre list behaviour (sort, letter navigation if any, `s` shuffles) kept; a List view (the 1.x rows) through the N5b Covers / List toggle.
- **Playlists**: cards with a 2×2 cover mosaic (or the playlist's own cover), name, song count; Covers / List likewise. **Mixes** keep their N12 cards, restyled to match (chips, Play, the playing mix marked).

### 5. Consistency pass (no new features)
- Every view and dialog uses the 2.0 tokens, type and shapes; no leftover 1.x styles (grep for dead classes and unused CSS); focus rings visible everywhere; keyboard paths unchanged.

## Acceptance
- Page transitions and cover morphs run on macOS and degrade cleanly; no element ever has a duplicate `view-transition-name` (add a dev-only assertion if cheap); no layout jump after Back; reduced motion means no animation anywhere.
- `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core`.
- Docs: `docs/DESIGN.md` (Motion *(shipped)*; every section's status accurate; the remaining owner items listed as open), `docs/ARCHITECTURE.md`, `README.md`, `scripts/smoke.md` ("2.0 N7 — Motion"), `docs/PLAN.md` (N7 status, the owner items still open).
- Do not commit; report changed files, decisions, checks, open items.
