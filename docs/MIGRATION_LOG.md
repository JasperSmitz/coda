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

## Phases
- [x] Plan (04e04d8), import (1dc4bce)
- [x] Phase 1 — workspace layout
- [x] Phase 2 — promote coda-core
- [ ] Phase 3 — TUI consumes coda-core
- [ ] Phase 4 — consolidation, docs, verification
