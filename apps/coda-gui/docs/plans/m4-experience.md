# Brief: experience — Now Playing view, media controls, settings/onboarding, platform fixes

Read first: `AGENTS.md`, `docs/DESIGN.md`, `docs/ARCHITECTURE.md` (what M1 built). Follow M1's patterns. Reference: `../coda/src/app.rs` (swap_client, SettingsIo, sync_visualizer), `../coda/src/ui/now_playing.rs`, `../coda/src/onboarding.rs` (read-only; never modify `../coda`).

A parallel slice (library browsing, search) is being built on another branch and also touches `engine.rs`, `dto.rs`, `types.ts`, `Sidebar.tsx`, `App.tsx`. To keep merges easy: put new code in new modules, add to shared files in small separate blocks, and don't reformat or reorganise code you don't need to change.

## 1. Now Playing view (`f`)
Full main-area view per DESIGN.md: large cover (`/large`), blurred darkened cover background, title/artist/album, timeline, transport, and **visualizer** bars below (from `visualizer` events; smoothing and the 300 ms stale decay done in the frontend, e.g. requestAnimationFrame + canvas; spread 7 bands into ~32–48 bars like `visualizer::spread_bands`). Send `SetVisualizerVisible(true)` only while the view is shown and the window is visible/focused-agnostic but not minimised. "Up next" line. Respect `prefers-reduced-motion`.

## 2. Media controls
OS integration via the `souvlaki` crate (MPRIS on Linux, MPNowPlayingInfoCenter/remote commands on macOS): play/pause/next/previous/seek from media keys and the desktop's media widgets; publish metadata (title, artist, album, duration, cover — a `file://` path from Coda's disk cache or the cover file is fine) and playback status/position. Lives in its own module fed by the engine's state; it sends intents like the UI does. On macOS it needs the main thread's run loop — follow souvlaki's docs for Tauri/winit. If it fails to initialise, log and continue.

## 3. Settings and onboarding
- Settings view (sidebar "Settings"): Navidrome URL/username/password with "Test connection" (ping) and Save, Disconnect; music folder (folder picker via `tauri-plugin-dialog`, minimal capability); cover art on/off; scrobbling on/off. Saving writes Coda's `config.toml` and the keychain exactly like Coda's settings (use `coda_core::setup_actions` / `config_writer` / `secret_store`). Note: the config file is shared with the Coda TUI — preserve unknown keys/comments the way `config_writer` does.
- Engine intents to apply changes live: swap the Navidrome client (port Coda's `swap_client`: new generation, reset scrobble state, new scrobbler, covers/browse caches invalidated, queued remote tracks resolve against the new client) and change the music folder (new library + rescan).
- Onboarding: when there's no config (`ConfigError::NoSource` problem), show a first-run screen instead of the empty shell: pick a music folder and/or connect Navidrome, then continue.

## 4. Platform and startup fixes
- If the GUI has no session file yet, import Coda's TUI session once (copy its state into the GUI session, never modify the TUI's file).
- mpv discovery: if `mpv` isn't found on `PATH`, also try `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin` (GUI apps launched from Finder/Dock get a minimal PATH). Change this in `coda-core`'s mpv spawn with a test.
- Honour `CODA_MUSIC_DIR` like the TUI does.
- Volume slider while stopped: remember the preferred volume instead of snapping back (use the player's preferred volume semantics; add to `coda-core` if needed, with tests).
- Remember window size/position (`tauri-plugin-window-state`).

## Acceptance
Checks from AGENTS.md all pass (`cargo fmt`, clippy `-D warnings`, `cargo test --workspace`, `pnpm build`). No window opened on the owner's desktop; add manual steps to `scripts/smoke.md`. Update `docs/ARCHITECTURE.md`. Don't commit. Report to `/tmp/coda-app-experience-report.md`.
