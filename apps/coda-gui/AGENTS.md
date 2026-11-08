# Coda App — Agent Operating Contract

## Project
Coda App (package `coda-app`) is the desktop frontend of Coda, the Rust music player for local files and a Navidrome server. It is a **native desktop app built with Tauri 2**, not a web application: no hosted pages, no server, no browser tab. Linux (Arch, the owner's main desktop) and macOS are supported; Windows is out of scope.

It lives in the Coda monorepo. The root `AGENTS.md` applies (roles, workflow, the dependency rule, verification); this file adds what is specific to the GUI. The engine is `crates/coda-core` at the repository root (lib `coda_core`), shared with the terminal player in `crates/coda-tui`. Change `coda-core` (with core tests, keeping the TUI building) when the GUI needs engine behaviour; never reimplement in the app what `coda-core` already does, and never depend on `coda-tui`.

## Layout (paths relative to `apps/coda-gui/`)
- `src-tauri/`: the Tauri backend (`coda-app`, lib `coda_app_lib`), a member of the root Cargo workspace. Owns one engine actor and exposes Tauri commands and events.
- `src/`: the React + TypeScript frontend (Vite). Draws state and sends intents. No business logic that the backend already owns.
- `scripts/`: `check-colors.mjs` (no colour values outside the token layer), `linux-check.sh` (Docker build and checks), `smoke.md` (the owner's runtime checklist).
- `packaging/`: the Arch PKGBUILD and the Linux desktop entry. CI: `.github/workflows/coda-gui-ci.yml` and `coda-gui-release.yml` at the repository root.
- `docs/DESIGN.md`: the look and layout. `docs/PLAN.md`: milestones and acceptance criteria. `docs/ARCHITECTURE.md`: what actually exists (label proposals as such).

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
- Same roles as the root `AGENTS.md`: owner approves scope; the architect (main session) writes briefs, delegates to implementer/reviewer agents, resolves findings and reports.
- Small vertical slices per milestone in `docs/PLAN.md`. Don't commit or push unless asked; the architect may commit an approved slice once review findings are resolved and checks pass. Never push without a request.

## Verification
- Rust, from the repository root: `cargo fmt --all -- --check`, `cargo clippy -p coda-app -p coda-core --all-targets -- -D warnings`, `cargo test -p coda-app -p coda-core` (mpv tests are `#[ignore]`d: add `-- --ignored`), or the workspace-wide commands from the root `AGENTS.md`. The Rust build embeds `dist/`, so run `pnpm build` first.
- Frontend, from `apps/coda-gui`: `pnpm install --frozen-lockfile`, `pnpm build` (runs the colour check, `tsc` strict and Vite) and `pnpm check:colors`.
- Release binary: `pnpm tauri build --no-bundle -- --locked --profile release-gui` (output in `../../target/release-gui/`). Never a release build without `--profile release-gui`: the default `release` profile unwinds, and the panic hook relies on `panic = "abort"` to take mpv down.
- Run the app with `pnpm tauri dev` only when the owner asks or for a hand-off script; agents must not open GUI windows on the owner's desktop unprompted. Backend logic is tested without a window; runtime checks are handed to the owner (`scripts/smoke.md`).
- Report commands actually run; never claim a check passed without running it.
