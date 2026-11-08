# Monorepo migration log

Brief: `docs/plans/monorepo-migration.md`. Branch: `monorepo-migration`. Newest entries last.

## Recovery points
- `pre-monorepo-coda` → `02a318b` (coda main = origin/main at start).
- `pre-monorepo-coda-app` → `a7614d9` (coda-app main = origin/main at start; fetched into this repo, branch `import/coda-app-main`).
- `/storage/development/coda-app` is untouched (only pre-existing untracked Arch packaging outputs in its tree).

## Baseline (before any change, 2026-11-08)
- coda: fmt, clippy `-D warnings`, `cargo test --all-targets` → 1011 passed, 34 ignored (mpv).
- coda-app: fmt, clippy, `cargo test --workspace` → 842 passed, 33 ignored (coda-core 660/30, Tauri 182/3); `pnpm build` ok.
  (The baseline's `pnpm install/build` left git-ignored `node_modules/`, `dist/` in the coda-app checkout; its git state is unchanged.)

## Decisions
- `coda` stays the canonical repository; coda-app imported with `git subtree add --prefix=apps/coda-gui` (no squash, 35 commits of history kept).
- The GUI's vendored `crates/coda-core` is a strict superset of coda's UI-free modules (see the brief), so it becomes the authoritative core; nothing is rewritten.

## Phase 1 — workspace layout
- TUI moved with `git mv` to `crates/coda-tui` (package `coda-tui`, lib `coda_tui`, bin `coda`); root virtual workspace; nested coda-app manifest/lock removed.
- One `Cargo.lock`: every coda-app entry kept; 23 TUI crates moved to coda-app's already-locked patch/minor versions (incl. lofty 0.25.4→0.25.5, ogg_pager 0.7→0.8). Unavoidable: one lock holds one semver-compatible version, and both packages require `lofty 0.25`.
- Release profiles: TUI keeps the default `release` (unwind; its Drop guards restore the terminal). GUI uses `[profile.release-gui]` (old coda-app settings incl. `panic = "abort"`, which its mpv-killing panic hook relies on). Tauri CLI 2.12.4 honours `-- --profile release-gui` (checked in its sources and with a real deb bundle). **Trade-off:** a plain `pnpm tauri build` / `cargo build --release -p coda-app` gets unwind + no LTO; every scripted release path (workflow, PKGBUILD, linux-check.sh, README) passes the flag.
- Tags: TUI releases stay `v*.*.*`; GUI releases now `coda-gui-v*.*.*` (both workflows used `v*` and would both fire). GUI drafts are published with `--latest=false`.
- CI: `ci.yml` (TUI, scoped `-p coda-tui` so it needs no webkit/dist) and `coda-gui-ci.yml` (`working-directory: apps/coda-gui`, `-p coda-app -p coda-core`), separate concurrency groups and rust-cache keys.
- Log targets renamed `coda_player::…` → `coda_tui::…` (CHANGELOG note).
- Verified: fmt; clippy workspace with/without `--all-features`; `cargo test --workspace --all-targets` 1853 passed / 67 ignored (= both baselines); `-- --ignored` 67/67 with mpv; `pnpm build`, `check:colors`; `tauri build --profile release-gui` (deb); TUI pty smoke (launch empty folder, `q`, exit 0, terminal restored, its mpv gone).
- Review: 0 blocking, 6 low. Fixed: shared rust-cache keys, `--latest=false`, CHANGELOG note, this log. Deferred to Phase 4: stale `coda-player` in AGENTS.md/ARCHITECTURE.md and workspace-wide commands in AGENTS.md.
- Not verified: Rust 1.89 MSRV (no rustup; CI msrv job), actionlint, makepkg, Docker linux-check, macOS, GUI at runtime.

## Phase 2 — promote coda-core
- `git mv apps/coda-gui/crates/coda-core crates/coda-core`; src-tauri path dep updated; "vendored" wording removed, crate docs describe the shared engine. No code change; lock unchanged.
- Boundary: `cargo tree -p coda-core -e normal --target all` has no ratatui/crossterm/tauri/souvlaki/wry/webkit/gtk/unicode-width.
- Visibility changes the TUI needs (e.g. `setup_actions::Reply`) moved to Phase 3, where the compiler names exactly what is needed.
- Accepted exception for now: `clap` stays in core for `config::Cli` (the TUI's argv; the GUI builds `Cli` by hand). Revisit in Phase 4.
- Verified: fmt; clippy (±all-features); core 660 passed/30 ignored standalone; workspace 1853/67; core `--ignored` 30/30; `cargo build -p coda-app`; `pnpm build`.
- Review: 0 blocking; doc nits fixed (lib.rs module list, CI comment).

## Phase 3 — TUI consumes coda-core
- coda-tui now holds only `app`, `setup`, `ui/**`, `main` (−39k lines). Every deleted module was diffed against core first: TUI-side differences were only behaviour-preserving refactors made in the GUI copy; nothing lost. `setup.rs` uses `coda_core::setup_actions`; `main.rs` uses `coda_core::logging` and `config::navidrome_client` (same messages and exit codes).
- Only core visibility change: `setup_actions::Reply` (+`new`/`send`) is `pub`. Test-only `expose()` accessors stayed `pub(crate)`; TUI tests compare values / use the public mpv encoder instead.
- Behaviour now reaching the TUI from core: `Drop for PlaybackHandle` and Linux `PR_SET_PDEATHSIG` (mpv is spawned on the main thread under `block_on`, so the signal lasts; SIGKILL of `coda` verified to take mpv down); `+`/`-` while stopped set the preferred volume (CHANGELOG).
- Frontend-specific on purpose: the TUI spawns mpv PATH-only (`mpv::spawn(MpvOptions::default())`, as before; its CLI tests rely on it); the GUI uses `spawn_backend()` with the Homebrew//usr/local fallback for desktop launchers.
- Features: core gained `test-util` (only exposes `SecretStore::mock()`), used by coda-tui's dev-dependency. Review caught that using `mock-secret-store` there made `cargo test` overwrite `target/*/coda` with a mock-keychain build; fixed and checked (`strings target/debug/coda` after `cargo test`: 0 mock strings).
- TUI CI also runs coda-core's clippy/tests/mpv tests (core needs no webkit).
- Tests: workspace 1211 passed / 37 ignored (core 660/30 and coda-app 182/3 unchanged; coda-tui 369/4). Of 673 removed TUI tests, 672 have a same-named test in core; the remaining one was renamed in core for the volume change. `-- --ignored` 37/37 with mpv.
- pty smoke (silent FLAC fixtures, temp HOME/XDG, mock-secret-store build): launch, scan, `-` while stopped, play, pause, seek, next, `+`, Now playing, visualizer toggle, artist→album→track navigation, quit → exit 0, terminal restored, no mpv left. Visualizer bars not visible (digital silence).
- Review: 1 blocking (feature unification, fixed), 2 low (CHANGELOG log targets, CI coverage — fixed), 2 nits (fixed / this entry).

## Phase 4 — consolidation and docs
- `[workspace.dependencies]` for crates used by 2+ members; Cargo.lock and resolved feature graphs unchanged (compared per package).
- Boundary checks clean: core has no UI crates; `cargo tree -i` shows neither frontend in the other's graph; no source references across frontends.
- Docs: root README (layout, building/running each frontend), root AGENTS.md rewritten for the monorepo (dependency rule, where code belongs, frontend-specific list, features, profiles, tags, per-package checks), GUI AGENTS.md trimmed to a GUI contract, both ARCHITECTURE files, GUI PLAN/README, `apps/coda-gui/scripts/smoke.md` owner hand-off checklist, ADR 0006, LESSONS entry (dev-dependency feature unification). `coda-app` gained `rust-version = "1.89"`.
- Verified: fmt; clippy ±all-features; workspace tests 1211/37; `--ignored` 37/37; `cargo build --workspace`; `cargo build --release -p coda-tui`; `pnpm build`, `check:colors`; `tauri build --no-bundle --profile release-gui`; TUI pty smoke; documented commands run (not `pnpm tauri dev`).
- Final whole-branch review: 0 blocking. History verified (`git log --follow` reaches original commits of both repos; all 268 coda-app files accounted for); no secrets in the diff. Doc nits fixed. Finding: the GUI engine still duplicates the TUI's scrobble policy and "Shuffle all" orchestration (inherited from coda-app) → Phase 5.
- Environment note: core tests that write a few MB can fail with `QuotaExceeded` when `/tmp` (tmpfs with quota) is full; set `TMPDIR` elsewhere.

## Phase 5 — remaining duplicated orchestration
- Moved into core, both frontends call it: `navidrome::scrobble::Scrobbling` (now-playing/submit listen policy, client swap, first-failure flag; GUI's live toggle = `switch`), `queue::shuffle_all` + `shuffle_all_message`, `model::count`, `Player::play_tracks` (play-in-context: shuffle-with-first, cap, dropped count). Same RNG consumption as before.
- Differences kept frontend-side on purpose: the GUI filters an out-of-range start before `play_tracks` (TUI starts are always in range); only the GUI toggles scrobbling live; "queue full" wording differs per frontend.
- Deferred (needs an owner decision): a shared session-save schedule. The two debounces share `note_changes`/`take_due_save`, but the GUI retries failed saves with back-off and the TUI only at exit; unifying would change TUI behaviour. Also tiny/not worth it: `Queue::navidrome_song_ids` for "save queue as playlist".
- Tests: core 660→674 (+14, mostly ported from the TUI's app tests), TUI 369 and GUI 182 unchanged in substance (call sites only); `--ignored` 37/37; TUI pty smoke (shuffle all, album shuffle/play, next, quit) with mpv `--ao=null`.
- Review: 0 blocking; architecture docs updated, `play_tracks` doc clarified, `derive(Clone)` dropped.

## Phases
- [x] Plan (04e04d8), import (1dc4bce)
- [x] Phase 1 — workspace layout
- [x] Phase 2 — promote coda-core
- [x] Phase 3 — TUI consumes coda-core
- [x] Phase 4 — consolidation, docs, verification
- [x] Phase 5 — move remaining duplicated orchestration (scrobble policy, shuffle all) into core
