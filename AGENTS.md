# Coda — Agent Operating Contract

## Project
Coda (`coda-player`, CLI: `coda`) is a standalone Rust terminal music player for local files and a Navidrome server. It may later become a music module for HEART, but must work independently.

## Source of truth
- `docs/VISION.md`: product goals, boundaries, and user experience.
- `docs/ARCHITECTURE.md`: actual architecture; clearly label anything only proposed.
- `docs/ROADMAP.md`: milestones and acceptance criteria.
- `docs/decisions/`: decisions and trade-offs, not speculative bureaucracy.
- `docs/plans/`: approved implementation briefs when needed.
- `docs/LESSONS.md`: verified, reusable lessons from defects and reviews.
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
7. Do not create a large agent framework, multiple crates, or extra documentation just to appear organized.

## Technical boundaries
- Rust stable; start as a single Cargo package unless a concrete need for a workspace emerges.
- Supported platforms: Linux and macOS. Windows is out of scope.
- Preferred TUI: Ratatui + Crossterm. Async tasks: Tokio.
- Initial audio engine: **mpv via JSON IPC**. Do not implement audio decoding or streaming from scratch. Hide mpv control behind a narrow playback boundary so it can be changed later.
- Music-domain models and queue/playback state must not depend on terminal widgets or HEART.
- Separate browsing/source discovery from playback. Tracks must retain local or Navidrome source identity while sharing a queue.
- Never block the TUI render/input loop on scans, HTTP requests, or player I/O.
- Prefer simple explicit types, small modules, and error handling over premature generic traits and abstractions.
- Avoid shell interpolation of user-controlled file names, paths, URLs, or metadata. Treat server responses and files as untrusted inputs.
- Never hardcode or commit Navidrome credentials, playback secrets, tokens, or private server URLs. Redact sensitive information from logs.
- Ensure terminal restoration and mpv process cleanup during exit and errors.

## Verification
Once a Cargo package exists, use the relevant checks:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets`
- `cargo build`
- Tests that need mpv are `#[ignore]`d; run `cargo test --all-targets -- --ignored` when mpv is installed.
- Manual smoke checks where practical: local playback, controls, missing files, empty library, terminal restoration.
Report commands actually run; never claim a check passed without running it. When unavailable, state why.

## Scope and documentation discipline
- Work in small vertical slices. Finish local playback (M1) before adding Navidrome (M2).
- HEART integration, persistence enhancements, artwork, MPRIS, equalizers, elaborate plugin systems, and database-backed library indexing are future work unless explicitly approved.
- Update `docs/ARCHITECTURE.md` for shipped structural changes, `docs/ROADMAP.md` for milestone status, and `docs/decisions/` for consequential choices.
- Add `docs/LESSONS.md` entries only for demonstrated, reusable lessons, not routine status notes.
- Keep responses concise: decisions, important trade-offs, verification, and next actions.
