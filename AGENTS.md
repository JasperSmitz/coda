# Coda — Agent Operating Contract

## Project
Coda is a Rust music player for local files and a Navidrome server, with one queue for both. This repository is a Cargo workspace with one engine and two frontends:

| Path | Package (lib / bin) | What |
|---|---|---|
| `crates/coda-core` | `coda-core` (`coda_core`) | The engine: models, queue, player, resolve, playback (mpv IPC), library, Navidrome, session, config (+ writer), secret store, cover art, now-playing and visualizer state, onboarding/settings state, setup actions, logging, line input. |
| `crates/coda-tui` | `coda-tui` (`coda_tui` / `coda`) | The terminal player: `app` (event loop), `setup` (first-run driver), `ui` (Ratatui drawing, keymap, terminal cover art), `main`. |
| `apps/coda-gui` | `coda-app` (`coda_app_lib`, in `src-tauri/`) + a React/Vite frontend in `src/` | The desktop app (Tauri 2). Its own contract: `apps/coda-gui/AGENTS.md`. |

It may later become a music module for HEART, but must work independently.

## Source of truth
- `docs/VISION.md`: product goals, boundaries, and user experience.
- `docs/ARCHITECTURE.md`: actual architecture of the workspace, the engine and the TUI; clearly label anything only proposed. The GUI's is `apps/coda-gui/docs/ARCHITECTURE.md`.
- `docs/ROADMAP.md`: TUI milestones; `apps/coda-gui/docs/PLAN.md`: GUI milestones.
- `docs/decisions/`: decisions and trade-offs, not speculative bureaucracy (ADR 0006: the monorepo and shared core).
- `docs/plans/`: approved implementation briefs when needed.
- `docs/LESSONS.md`: verified, reusable lessons from defects and reviews.
- `README.md` (overview, building, TUI keys), `docs/USAGE.md` (TUI user-facing details) and `apps/coda-gui/README.md`: update them when user-visible behaviour changes.
- Inspect the repository before asserting what exists. Code and tests prevail over outdated docs; reconcile discrepancies.

## Agent roles
- **Owner (user):** sets priorities and explicitly approves implementation scope.
- **Architect (main Claude session):** discusses designs, inspects code, challenges scope, creates briefs, delegates approved work, coordinates review/fixes, and reports back.
- **Developer (`.claude/agents/developer.md`):** implements only the approved task, writes tests, verifies, and reports changes.
- **Reviewer (`.claude/agents/reviewer.md`):** independently examines the diff and relevant surrounding code, identifies actionable findings, and reports evidence. Does not change source files.
- **Models:** developer and reviewer default to Opus. The architect may run the developer on Sonnet for small, focused follow-ups.

## Operating workflow
1. **Discussion is the default.** Analysis, read-only exploration, and proposals are welcome. Do not modify files, install dependencies, or delegate implementation until the owner explicitly asks to implement or scaffold a specified change.
2. Before proposing or coding: read relevant docs, inspect the actual repository, and distinguish implemented behavior from proposals.
3. For nontrivial work: architect presents a focused scope and acceptance criteria, then awaits owner approval. A direct request such as “implement M1” grants approval for that named scope.
4. On approval: architect delegates to developer; developer implements and runs the relevant checks; architect calls reviewer for an independent read-only review.
5. Architect resolves review findings with targeted developer follow-up, re-verifies, updates documentation, and reports what changed, what was tested, and what remains.
6. Do not commit, push, delete user data, or take unrelated actions unless explicitly requested. Exception: the architect may commit an approved slice once review findings are resolved and checks pass. Never push without a request.
7. Do not create a large agent framework, extra crates, or extra documentation just to appear organized.

## Dependency rule and where code belongs
- `coda-core` depends on no frontend and no UI toolkit (no ratatui, crossterm, tauri, souvlaki, webview crates). `coda-tui` and `coda-app` depend on `coda-core` and never on each other. Check with `cargo tree -p coda-core -e normal --target all` and `cargo tree -p <frontend> -i <other frontend>` (must not match).
- Business logic belongs in `coda-core`: queue and player rules, playback and gapless logic, sources, session, config, credentials, setup/settings state machines and their I/O actions. A frontend that needs engine behaviour changes core (with core tests) instead of reimplementing it. Make an item `pub` only when a frontend needs it.
- Frontend-specific on purpose (see `docs/ARCHITECTURE.md`): TUI drawing, keymap, terminal cover rendering, its setup/app event loops, PATH-only mpv lookup; GUI engine actor, DTOs, Tauri commands, browse/history/playlist caches, media keys, themes, its own session and log under `coda/coda-app/`, mpv fallback lookup. The visualizer's measurement is in core; each frontend draws it.
- Accepted exception: `clap` stays in core for `config::Cli` (the TUI's command line; the GUI fills `Cli` by hand).
- Shared dependency versions live in the root `[workspace.dependencies]`; members add their own features. Do not run `cargo update` as a side effect.

## Technical boundaries
- Rust stable, MSRV 1.89 (`rust-version`). One workspace, one `Cargo.lock`.
- Supported platforms: Linux and macOS. Windows is out of scope.
- TUI: Ratatui + Crossterm. GUI: Tauri 2 + React 19 + TypeScript (strict) + Vite. Async tasks: Tokio.
- Audio engine: **mpv via JSON IPC**, behind the narrow playback boundary in `coda_core::playback`. Do not implement audio decoding or streaming from scratch.
- Music-domain models and queue/playback state must not depend on UI code or HEART.
- Separate browsing/source discovery from playback. Tracks must retain local or Navidrome source identity while sharing a queue.
- Never block a frontend's render/input loop (or the GUI's engine actor) on scans, HTTP requests, or player I/O.
- Prefer simple explicit types, small modules, and error handling over premature generic traits and abstractions.
- Avoid shell interpolation of user-controlled file names, paths, URLs, or metadata. Treat server responses and files as untrusted inputs.
- Never hardcode or commit Navidrome credentials, playback secrets, tokens, or private server URLs. Redact sensitive information from logs.
- Ensure terminal restoration and mpv process cleanup during exit and errors, in both frontends.
- Features: `mock-secret-store` (core, forwarded by coda-tui) swaps the OS keychain for an in-memory store, for pty smoke builds only; never enable it in a release or through a dev-dependency (Cargo would unify it into the `coda` binary that `cargo test` leaves in `target/`, LESSONS). Tests in other crates use core's `test-util`, which only exposes `SecretStore::mock()`.
- Release builds: the TUI uses the default `release` profile (unwind, which its terminal-restoring Drop guards need). The GUI uses `[profile.release-gui]` (LTO, `panic = "abort"`, which its mpv-killing panic hook needs): always `pnpm tauri build … -- --profile release-gui`. Tags: `v*` releases the TUI (`release.yml`), `coda-gui-v*` the GUI (`coda-gui-release.yml`, draft).

## Verification
From the repository root:
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` (`--all-features` swaps in the mock secret store, so lint both ways)
- `cargo test --workspace --all-targets`; tests that need mpv are `#[ignore]`d: `cargo test --workspace -- --ignored` when mpv is installed.
- `cargo build --workspace`
- The workspace commands build the GUI too, which needs the Tauri system libraries and a built frontend (`apps/coda-gui/dist`, from `pnpm build`). Without them, check the engine and TUI only: `cargo clippy -p coda-tui -p coda-core --all-targets -- -D warnings` and `cargo test -p coda-tui -p coda-core --all-targets` (as `.github/workflows/ci.yml` does).
- GUI frontend: `cd apps/coda-gui && pnpm install --frozen-lockfile && pnpm build && pnpm check:colors`. GUI release binary: `pnpm tauri build --no-bundle -- --locked --profile release-gui`.
- Manual smoke checks where practical: local playback, controls, missing files, empty library, terminal restoration. TUI pty smoke runs use a `--features mock-secret-store` build with temporary `HOME`/`XDG_*` directories. GUI runtime checks are handed to the owner (`apps/coda-gui/scripts/smoke.md`).
- Agents run TUI and terminal probes only under a pty they own (stdin/stdout/stderr on it, scripted replies, hard timeout). Never launch terminal emulators or other GUI apps on the owner's desktop (`open -a`, `open -na`, `osascript`, `.command` files, `pnpm tauri dev` unless asked) and never write to `/dev/tty`; checks that need a real terminal or window are handed to the owner as a script to run (see LESSONS).
Report commands actually run; never claim a check passed without running it. When unavailable, state why.

## Scope and documentation discipline
- Work in small vertical slices, one frontend or one core change at a time, with both frontends still building.
- HEART integration, daemons, equalizers, elaborate plugin systems, and database-backed library indexing are future work unless explicitly approved.
- Update `docs/ARCHITECTURE.md` (or the GUI's) for shipped structural changes, `docs/ROADMAP.md` / `apps/coda-gui/docs/PLAN.md` for milestone status, and `docs/decisions/` for consequential choices.
- Add `docs/LESSONS.md` entries only for demonstrated, reusable lessons, not routine status notes.
- Keep responses concise: decisions, important trade-offs, verification, and next actions.
