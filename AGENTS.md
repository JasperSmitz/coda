# Coda App — Agent Operating Contract

## Project
Coda App (`coda-app`) is a desktop GUI for [Coda](../coda), the Rust music player for local files and a Navidrome server. It is a **native desktop app built with Tauri 2**, not a web application: no hosted pages, no server, no browser tab. Linux (Arch, the owner's main desktop) and macOS are supported; Windows is out of scope.

Coda's Rust library (`coda-player`, lib `coda_player`) is the engine: queue, player, gapless mpv playback, Navidrome client, local library scan, session, cover art, visualizer data, credentials. **This repo replaces only Coda's terminal UI.** Never reimplement what the library already does; if the GUI needs something the library hides, add it to Coda (small, reviewed change on a branch there) rather than copying it here.

## Layout
- `src-tauri/`: the Rust backend. Depends on `coda-player` with `default-features = false` (no Ratatui/Crossterm). Owns one engine actor and exposes Tauri commands and events.
- `src/`: the React + TypeScript frontend (Vite). Draws state and sends intents. No business logic that the backend already owns.
- `docs/PLAN.md`: milestones and acceptance criteria. `docs/ARCHITECTURE.md`: what actually exists (label proposals as such).

## Architecture rules
- **One engine actor** in the backend owns `Player`, the `PlaybackHandle`, the Navidrome client, library and session state, mirroring Coda's single `tokio::select!` loop. Tauri commands send intents to it over a channel; it emits events. No shared `Mutex<Player>` poked from command handlers.
- Intents go through `Player` and `resolve::dispatch`, never raw `PlaybackCommand`s from the UI (gapless/stale-load logic depends on it). Call `dispatch` after every queue edit.
- The frontend receives **serializable DTOs** defined in the backend (serde), never library types directly. Keep one generated or hand-maintained TypeScript mirror in `src/lib/types.ts`.
- State to the frontend: a full snapshot on change (debounced/coalesced) plus a lightweight high-rate stream for position and visualizer frames. The frontend never polls.
- Never block the engine on I/O: scans, HTTP, cover loads and session writes run on tasks or blocking threads, results come back as messages.
- On window close/quit: final bounded session save, then `PlaybackHandle::shutdown`. mpv must never outlive the app.
- Security: Tauri capabilities stay minimal (only what the app uses); a strict CSP; no remote content in the webview except images served through our own commands/protocol. Never log or send to the frontend passwords, tokens or signed stream URLs. Treat server data and file tags as untrusted (React escapes text; never use `dangerouslySetInnerHTML`).

## Frontend
- React 19 + TypeScript (strict) + Vite. Plain CSS with design tokens (CSS custom properties) in `src/styles/`; no UI framework unless the owner approves one.
- Keyboard-first like Coda: every main action has a shortcut, and focus is always visible. Mouse and drag-and-drop are first-class too.
- Light and dark themes from the same tokens.

## Workflow
- Same roles as Coda: owner approves scope; the architect (main session) writes briefs, delegates to implementer/reviewer agents (herdr `hc` templates), resolves findings and reports.
- Small vertical slices per milestone in `docs/PLAN.md`. Don't commit or push unless asked; the architect may commit an approved slice once review findings are resolved and checks pass. Never push without a request.
- Changes to `../coda` happen on a branch there, follow Coda's own AGENTS.md, and keep the TUI's behaviour unchanged.

## Verification
- `cd src-tauri && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test`
- `pnpm build` (runs `tsc` strict + Vite) and `pnpm lint` once a linter exists.
- Run the app with `pnpm tauri dev` only when the owner asks or for a hand-off script; agents must not open GUI windows on the owner's desktop unprompted. Backend logic is tested without a window.
- Report commands actually run; never claim a check passed without running it.
