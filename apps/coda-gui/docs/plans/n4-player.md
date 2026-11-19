# Brief N4 — Player, queue and Now Playing (2.0, Neo-Coda)

Approved by the owner (2026-11-19). Design: `docs/DESIGN.md` "Player", "Queue", "Now Playing", "Motion". Builds on N1 (queue panel above the docked bar), N2 (tokens, glass), N3 (the bar's cover background and the playing palette scope).

## Goal
The docked 1.x bar becomes the floating **player** strip; the queue rises out of it; opening Now Playing morphs the player's cover into Now Playing's cover.

## Scope

### 1. The floating player (`components/NowPlayingBar.tsx` → the player; keep the file or rename, update imports)
- A rounded strip (~26px radius) floating ~16px above the bottom edge, centred, `width: min(1000px, 100% - 32px)`, ~76px tall; the page scrolls under it (the main view's bottom padding keeps the last row reachable, as N1 did for the top bar).
- Layout: left the cover (54px, rounded; click opens Now Playing windowed; Tab stop) and title / "artist · album" (links as in 1.x); centre previous / play-pause (filled circle) / next above the timeline (elapsed, seek bar, total, repeat after the total); right a small visualizer (5 bars, playing accent; gated like the existing visualizer so frames only flow while it is visible), volume, Queue, Full screen (Now Playing). Every 1.x bar control and tooltip stays.
- Background: glass with the playing cover enlarged and blurred under a `--tint` wash (from N3), crossfading on track change; text contrast holds over it.
- Narrow windows: below ~860px drop the visualizer and volume; below ~600px (if reachable) the timeline and previous/next.
- Nothing playing / empty queue: the strip stays with a quiet placeholder ("Nothing playing") and disabled controls, as the 1.x bar did.

### 2. The queue rises out of the player
- The N1 queue panel re-anchors to the player: right edge aligned with the player's right edge, its bottom ~10px above the player, ≤400px wide, up to ~560px / 60% tall; tinted glass (`--tint`). Opening: a short rise and scale from the queue button (`transform-origin` near the button, ~0.5s `--spring`), closing faster (~0.25s fade). Reduced motion: none. All N1 behaviour (Esc, `q`, button, outside click rules, focus in/out, remembered state, no drag region) and all queue features (selection, drag-and-drop, header buttons, mix line) unchanged.

### 3. Now Playing morph
- Opening Now Playing (cover click, `f`, the Full screen button) runs as a View Transition where the player's cover and Now Playing's cover share a `view-transition-name`, so the cover grows into place while the rest crossfades; closing reverses it. Use the existing `lib/motion.ts` `viewTransition` helper and its fallback (a short crossfade without View Transitions). Full screen entry (`f`) must not break: if entering full screen and the transition interfere, enter full screen after the transition finishes or skip the morph for the full-screen path — decide and say which. Track-change and lyrics-mode transitions inside Now Playing are unchanged; make sure their names do not collide with the new one.
- Now Playing itself keeps its layout; its side-panel queue is unchanged. Opening scale (0.98 → 1) stays for the fallback.

### 4. Cleanup
- Remove the docked bar's CSS, `--bar-height` uses that only served the dock, and any N1 temporary positioning for the queue panel.

## Out of scope
Home, Library, page transitions (N5–N7). Engine changes.

## Acceptance
- Every 1.x bar behaviour works in the player (controls, seek drag and click, volume, repeat, links, keyboard focus, disabled states while starting / mpv missing).
- The queue opens and closes from the player as specified; drag-and-drop and context menus inside it work; outside clicks close it.
- The morph runs on macOS (WKWebView has View Transitions) and degrades on WebKitGTK without them; Now Playing's own smoke checks (1.x items MO1–MO10, L1–L8) still pass.
- Nothing hides under the player at the end of any scrolling view; 900×600 works.
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core`.
- Docs: `docs/ARCHITECTURE.md`, `docs/DESIGN.md` (Player, Queue, the morph *(shipped)*), `README.md` if it describes the bar, `scripts/smoke.md` ("2.0 N4 — Player" section).
- Do not commit; report changed files, decisions, checks, open items.
