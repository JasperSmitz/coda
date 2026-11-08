# Monorepo migration: `coda` + `coda-app` with a shared `coda-core`

Status: approved by the owner (2026-11-08), in progress on branch `monorepo-migration`.
Log: `docs/MIGRATION_LOG.md`.

## Starting point (verified)

| Repo | Commit | Notes |
|---|---|---|
| `coda` | `02a318b` (main, = origin/main), tag `pre-monorepo-coda` | Single package `coda-player`: lib `coda_player` + bin `coda`. UI-free modules plus `app`, `setup`, `ui` (the only Ratatui/Crossterm users). |
| `coda-app` | `a7614d9` (main, = origin/main), tag `pre-monorepo-coda-app` in this repo | Workspace: `crates/coda-core` (vendored copy of coda's UI-free modules, taken at a `gui-core` branch that no longer exists) + `src-tauri` (Tauri 2) + React/Vite in `src/`. Untracked Arch packaging outputs in its tree are not imported. |

Key finding: `coda-app/crates/coda-core` is a **strict superset** of coda's UI-free library. Every difference is
an addition or a refactor made for the GUI, none is a coda-side fix missing from the copy:

- additions: `logging` (moved out of `main.rs`), `setup_actions` (UI-free half of `setup.rs`: `Reply`, folder
  check, ping, saves), `config::navidrome_client`, `config_writer::{read,write}_preferences`, `cover::load`/`encode_jpeg`,
  `Library::track_id_by_path`, `LibraryIndex::{artist_of, album_of, search}`, Navidrome album/artist/playlist cover and
  artist ids, `Client::album_detail`, `Player::{seek_absolute, move_entries}`, `Queue::move_entries`, `session::read_valid`;
- robustness: `Drop for PlaybackHandle` (SIGTERM mpv if never shut down), Linux `PR_SET_PDEATHSIG` on mpv, mpv lookup
  in Homebrew/`/usr/local/bin` when not on `PATH`;
- one behaviour change: `Player::set_volume` is public, clamps, and works while stopped (the TUI's `+`/`-` while
  stopped will now change the preferred volume instead of doing nothing).

So "extraction" means: promote the GUI's copy to the one authoritative `coda-core`, then make the TUI consume it and
delete the TUI's duplicates. No rewrite.

## Target layout

```
coda/
├── Cargo.toml            workspace: crates/*, apps/coda-gui/src-tauri; one Cargo.lock
├── crates/
│   ├── coda-core/        lib `coda_core`: model, queue, player, resolve, playback (mpv IPC), library, navidrome,
│   │                     session, config(+writer), secret_store, cover, now_playing, visualizer, onboarding,
│   │                     settings state, setup_actions, logging, line_input
│   └── coda-tui/         package `coda-tui`, lib `coda_tui` (app, setup, ui) + bin `coda`
├── apps/coda-gui/        the former coda-app tree (package.json, src/, src-tauri/, docs/, packaging/, scripts/)
├── docs/                 shared + TUI docs; GUI docs stay in apps/coda-gui/docs
├── .github/workflows/    CI for both frontends; both release workflows (paths adjusted, not run)
└── AGENTS.md, CLAUDE.md
```

Dependency rule: `coda-core` depends on neither frontend (no ratatui, crossterm, tauri, souvlaki); `coda-tui` and
`coda-gui` depend on `coda-core` only, never on each other.

What stays frontend-specific (intentionally): TUI rendering, keymap, terminal cover rendering, the TUI's
`setup`/`app` event loops; the GUI's engine actor, DTOs, Tauri commands, browse/history/playlists caches, media keys
(souvlaki), themes, its own session file (`$XDG_STATE_HOME/coda/coda-app/`) and log. Visualizer: the band
measurement and interpolation-to-bars state stays in `coda-core::visualizer` + `playback::mpv::visualizer`; each
frontend draws it its own way. No behaviour change.

## Phases (each: implementer → checks → independent review → fixes → commit)

### Phase 1 — Establish the monorepo (no logic extraction)
1. `git subtree add --prefix=apps/coda-gui import/coda-app-main` (full history, no squash).
2. `git mv` coda's package into `crates/coda-tui` (src, tests, Cargo.toml); package renamed `coda-tui`, lib
   `coda_tui`, bin stays `coda`, version stays 1.0.0.
3. Root virtual workspace: members `crates/coda-tui`, `apps/coda-gui/crates/coda-core` (still in place),
   `apps/coda-gui/src-tauri`. Remove the nested workspace manifest; merge the lock files (`cargo` regenerates; keep
   existing versions, no `cargo update`).
4. Release profile: keep both apps' release behaviour. coda-app uses `lto`, `codegen-units = 1`, `strip`,
   `panic = "abort"` (its panic hook relies on abort to kill mpv); coda used defaults (unwind, which its
   terminal-restoring Drop guards rely on). `panic` cannot differ per package within one profile, so put the GUI's
   settings in a custom profile (e.g. `release-gui`, `inherits = "release"`) and make the GUI's build use it if
   Tauri's CLI allows; otherwise choose and record the trade-off in the log.
5. Fix relative paths: `src-tauri` → core path, `tauri.conf.json` frontend paths, scripts, packaging, `.gitignore`
   (merge), CI workflows moved to the root `.github/workflows` with working directories adjusted
   (`coda-gui-ci.yml`, `coda-gui-release.yml`), coda's `release.yml` package-name lookup.

Acceptance: `cargo fmt/clippy/test` for the whole workspace pass; `pnpm build` in `apps/coda-gui` passes; `coda`
binary builds and launches in a pty; the GUI binary builds (`cargo build -p coda-app`). Test counts match the
baseline.

### Phase 2 — Promote `coda-core`
1. `git mv apps/coda-gui/crates/coda-core crates/coda-core`; update paths.
2. Make it the authoritative core: drop "vendored" wording, update crate/module docs, keep all its tests.
3. Make the items the TUI will need public where they are `pub(crate)` (e.g. `setup_actions::Reply`, `INTERRUPTED`)
   — only what is actually needed.
4. Confirm the core's tests cover everything the TUI's copy tested (the TUI's `tests/*` duplicates are identical up to
   the crate name; core-only additions already have tests).

Acceptance: core builds and tests standalone (`cargo test -p coda-core`), `cargo tree -p coda-core` shows no ratatui,
crossterm, tauri, souvlaki, futures-util-for-terminal, unicode-width-for-UI; both frontends still build.

### Phase 3 — TUI consumes `coda-core`
1. `coda-tui` depends on `coda-core`; delete its duplicate modules (everything except `app`, `setup`, `ui`, `main`);
   imports become `coda_core::…`. `setup.rs` uses `coda_core::setup_actions` instead of its own copies of
   `Reply`/`check_folder`/`ping`/`save*`; `main.rs` uses `coda_core::logging` and `config::navidrome_client` where
   behaviour is identical.
2. Delete the TUI's duplicate integration tests (now in core); keep `cli.rs`, `cli_music_dir_env.rs`,
   `cover_terminal.rs`, `panic_hook.rs` and every TUI unit test.
3. Adopt the core's `set_volume` behaviour (volume keys while stopped change the preferred volume); update the TUI's
   tests/docs for it.
4. Remove TUI dependencies that only the deleted modules used.

Acceptance: no duplicate implementation of queue, player, mpv, Navidrome, library, cover, session, config in
`crates/coda-tui`; all tests pass; `coda` launches, navigates, quits cleanly with no mpv left (pty smoke run with
`--music-dir` fixture and the mock secret store); GUI unaffected.

### Phase 4 — Consolidation and docs
- Dependency boundary check (`cargo tree`), shared dependency versions consistent.
- Root README (both frontends, how to run each), AGENTS.md (monorepo rules; remove "never modify ../coda"),
  `docs/ARCHITECTURE.md` (workspace + core), GUI `AGENTS.md`/`ARCHITECTURE.md` path updates, ADR
  `0006-monorepo-shared-core.md`, `MIGRATION_LOG.md`.
- Full verification (below).

## Verification matrix

Automated: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` (also with
`--all-features` for the TUI/core mock secret store); `cargo test --workspace --all-targets`; ignored mpv tests
(`-- --ignored`, mpv is installed); `pnpm build` + `pnpm check:colors`. Navidrome is tested against `wiremock`
fixtures only.

Runtime: TUI pty smoke (launch, navigate, play a local fixture with mpv, quit, check no mpv remains). GUI: no
display server is available to agents (no Xvfb) and agents must not open windows on the owner's desktop, so GUI
launch/navigation, real Navidrome and audio output are **not** agent-verified; a hand-off checklist is provided.

## Not in scope
HEART integration, daemons/IPC services, plugin frameworks, new features, pushing, releases, archiving `coda-app`.
