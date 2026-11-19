# Brief N1 — Shell and navigation (2.0, Neo-Coda)

Approved by the owner (2026-11-19: "start running until the design is implemented"). Design: `docs/DESIGN.md` "Layout", "Navigation", "Queue" (placement only). ADR 0009. Mockup (behaviour reference only, not code to copy): https://claude.ai/artifact/VpXdkrF5jpKBoNJ8va2Ez3.

## Goal
Replace the 1.x shell (sidebar + main + queue column + bar) with the 2.0 shell: a top bar with Home / Search / Library, Library as one root with sections, Preferences behind a button, the queue as a panel that opens above the bar. **No visual redesign beyond the shell**: the existing tokens and views stay (N2 brings the new look); the new shell parts use existing tokens (`color-mix` of tokens for translucency is fine; no colour literals, `check:colors` must pass).

## Scope

### 1. Navigation model (`src/state/nav.ts`)
- Roots: `home`, `search`, `library`, `prefs`. **One back stack per root, per source** (`stacks[source][root]`), plus the active root per source. Switching roots shows that root's stack where it was left (scroll positions kept as today). Clicking the active root's entry again pops its stack to the root (like a tab bar).
- `library`'s root route is the chosen **section**: `albums` (default), `artists`, `genres`, `playlists`, `mixes`, `folders` (local only; on Navidrome it falls back to albums). Changing section replaces the library stack with that section's root route. The chosen section is remembered per source (localStorage, as the queue flags are; survives a missing storage).
- Pushes (views opening an artist/album/genre/playlist, "Go to album/artist" from the bar, Now Playing or the queue) go onto the **active root's** stack; if the active root is `prefs`, they go to `library` and switch to it.
- `nav.search(query)`: switches to the `search` root and sets/focuses its query (not a push onto another root).
- Back (`Backspace`, `Alt+←`, mouse back, Esc when nothing is open, a view's Back) pops the active root's stack; at a root it does nothing (unchanged behaviour).
- `sourceReplaced(source)` resets all of that source's stacks to their roots. `setSource` shows the other source's active root and stack; the old `keepSettings` case becomes: if `prefs` is active, it stays active on the other source.
- Keep the logic pure and testable: put the state machine in a module without React/DOM (e.g. `src/lib/navModel.ts`) with `scripts/nav.test.mjs` covering: per-root stacks, re-click pops to root, push from prefs goes to library, section change, folders on Navidrome, source switch with prefs, sourceReplaced. Follow how the existing `scripts/*.test.mjs` import TypeScript helpers.
- Remove `viewFor`/`ViewId` usage that only served the sidebar (keep what still has a use).

### 2. Top bar (new component, e.g. `components/TopBar.tsx`)
- Grid: left the coda sign (an inline SVG in `icons.tsx`: an upright oval crossed by a vertical and a horizontal line, the music "coda" sign) + "coda" wordmark; centre a pill with **Home**, **Search**, **Library** (icon + label; labels hidden below ~860px wide), the active entry marked by a lighter pill that glides between entries (`transform`/`width` on one indicator element; no layout animation of the buttons; reduced motion: no glide); right a round **Preferences** button (sliders icon), marked when `prefs` is active. `aria-current="page"` on the active entry; buttons are real buttons; tooltips show shortcuts where they exist.
- macOS: the bar is the drag region (`dragRegion`), and the native window buttons sit in the bar's row, vertically centred on the pill: update `titlebar.rs` `BUTTONS` and the CSS so they stay in step (one comment in each pointing at the other, as today); the brand starts right of the buttons. In full screen (no buttons) the brand moves back to the left padding (the existing full-screen detection, if any; else leave the gap).
- Linux: no drag region (the WM's title bar stays); same layout.
- The bar floats over the page: the page scrolls under it, with a soft fade of the background colour under the bar so content reads as passing beneath. The page's content gets top padding so nothing is hidden at scroll top.

### 3. Library header
- When the library stack is at its root, the page shows a header: "Library" title and a segmented control of the sections (radio-group semantics, arrow keys move within it). The section views keep their own tools (sorts, Shuffle, New mix…); drop a section view's own big title only where it would now duplicate the control. Pages deeper in the stack (an artist, an album) show no section control.

### 4. Source and status out of the sidebar
- The sidebar is removed (component, CSS, tokens that only it used).
- The source switch ("Local | Navidrome", `Ctrl/Cmd+1`/`2` unchanged) and its status (the scan line "N tracks" / "Scanning… N" / "Scan failed", the Navidrome connection state) move into Settings as its first section, "Music source", shown when both sources are configured; with one source only its name and status. Settings keeps everything else as is (N2 restyles it as Preferences).

### 5. Queue placement (temporary until N4)
- The queue column is removed. `q` / the bar's queue button open the same `QueuePanel` as a floating panel **above the bar, anchored to the bar's right edge** (~380px wide, up to ~60% of the window height), opening with a short rise+fade (`--motion*` tokens, reduced motion: none). Esc, `q`, the button, or a pointer-down outside it closes it (not while a drag from it or a context menu/dialog from it is in progress). Focus moves into it on open (to its list) and back to the queue button on close if focus was inside. Its open/closed state is remembered as the old column's was. Now Playing's side-panel queue is unchanged.

### 6. The bar
- The 1.x now-playing bar stays docked at the bottom for N1 (N4 replaces it). Only adjust what the removed columns require.

## Out of scope
Tokens, fonts, colours, Home, Library sorts/views, the floating player, cover palettes, Now Playing (all later slices). No backend change except `titlebar.rs`.

## Acceptance
- Every 1.x view is reachable from Home / Search / Library / Preferences; per-root back stacks behave as above; "Go to album/artist" from the bar, Now Playing and the queue land on the right page; Search's "Search … instead" and cross-source "Go to" still switch sources first.
- All global shortcuts work; focus is always visible; the top bar's entries are keyboard reachable; nothing is hidden under the top bar or the bar at either end of a scroll; the minimum window (900×600) works.
- `nav.test.mjs` passes with the cases above.
- Checks (report the commands run): from `apps/coda-gui`: `pnpm build`, `pnpm check:colors`; from the repo root: `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core`.
- Docs: `docs/ARCHITECTURE.md` (navigation model, top bar, queue panel, sidebar gone), `docs/DESIGN.md` (mark "Layout" and "Navigation" *(shipped)* where true; note the bar is still the 1.x bar until N4), `apps/coda-gui/README.md` if it describes the sidebar, and `scripts/smoke.md`: a new "2.0 N1 — Shell" section (numbered N1-1…) with the runtime checks for the owner, and fix hand-off items that mention the sidebar.
- Do not commit; report changed files, checks run and anything left open.
