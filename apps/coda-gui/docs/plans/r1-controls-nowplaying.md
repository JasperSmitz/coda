# Brief (owner feedback round 1, slice A): transport controls and immersive Now Playing

Read first: `AGENTS.md`, `docs/DESIGN.md` (incl. theming: every colour is a token, `pnpm build` enforces it), `docs/ARCHITECTURE.md`. Another agent is building slice B (source switch + Home) in parallel on another branch; it touches `Sidebar.tsx`, browse views, `App.tsx` routing and the engine (history). Keep your changes contained; don't reorganise shared files.

## Owner's words
> I'd like repeat to be a button rather than pressing R and a banner appears; that's a bit confusing. Just have Repeat be an icon beside the timeline bar. Same thing for shuffle, have it be an icon above the queue. Save should be the playlist icon (or something similar) with a plus, clearing it should either be a broom icon or the playlist icon with a cross.
> Going into now playing mode by pressing f should hide all UI, having two timelines, play/pause, next/previous buttons is a bit confusing. Maybe the queue can pop in as a card below, or as a side panel.

## Decisions
1. **Repeat** is an icon toggle placed directly beside the timeline (right of the total time) in the now-playing bar: three visual states (off: muted icon; all: accent icon; one: accent icon with a small "1"), tooltip and `aria-label` naming the current state and the next. `r` still cycles it but **no notice/toast** for repeat changes (suppress the player's repeat message, wherever it comes from). Remove the old repeat control from its previous spot.
2. **Queue header** gets icon buttons with tooltips (left to right): **Shuffle upcoming** (shuffle icon), **Save queue as playlist** (playlist icon with a plus; only when Navidrome is connected, as now), **Clear queue** (a broom icon; keeps its confirmation). Replace any text buttons/menus that did these. Icons drawn in the existing `icons.tsx` style (24px grid, `currentColor`, 1.75 stroke).
3. **Now Playing (`f`) is immersive**: the sidebar, browse area, queue panel and the bottom now-playing bar are hidden (not just covered); only the Now Playing view remains, with ONE timeline, ONE transport row (shuffle-upcoming? no: previous / play-pause / next), repeat beside its timeline, volume, and a close button (Esc / `f`). Window-drag region and traffic lights on macOS must still work.
   - **Queue as a side panel** inside Now Playing: slides in from the right over the blurred backdrop (a translucent surface card, ~360px), toggled with `q` or a queue icon in the view; it is the same `QueuePanel` component (selection, drag-and-drop, header buttons all work). Its open/closed state is remembered separately from the normal layout's queue panel.
   - A small **"Up next"** card (next track's cover, title, artist) below the transport when the panel is closed; clicking it opens the panel.
   - Layout adapts to window size (cover shrinks; panel overlays rather than squeezes on narrow windows). Transitions short and respect reduced motion.
4. Shortcuts stay global in Now Playing (space, n/p, ←/→, +/-, r, q, Esc).

## Acceptance
All AGENTS.md checks (`cargo fmt`, clippy `-D warnings`, `cargo test --workspace`, `pnpm build` incl. the colour check). No window opened; update `scripts/smoke.md` and `docs/DESIGN.md`. Don't commit. Report to `/tmp/coda-app-r1a-report.md`.
