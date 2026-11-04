# Release 1.0: brief

- **Status:** approved by owner 2026-11-03. The owner wants a GitHub CI check and "a versioning thing that creates releases automatically", plus the small items. "Feel free to spin up the required subagents."
- **Owner verification (2026-11-03):**
  - **Ghostty and macOS Terminal.app:** the owner tested M9 and M10 in both. In Terminal.app the cover shows as half blocks, as intended.
  - **Linux:** tested up to M8, "works perfectly".
  - **Navidrome:** used exclusively with the owner's server, "works great".
- **Goal:** a clean, versioned 1.0 that builds, tests and releases itself from GitHub.

## Work items
1. **Version and docs.**
   - Set `Cargo.toml` to `1.0.0`.
   - README status: 1.0, M1–M10 done.
   - Add `CHANGELOG.md` (Keep a Changelog style) with a 1.0.0 entry, summarised from `docs/DEVELOPMENT_LOG.md` by feature, not by milestone.
   - ROADMAP: M9 and M10 marked as checked by the owner, and a "1.0" note.
2. **Colours that follow any theme.**
   - Dim text uses `Modifier::DIM` on the default foreground instead of `Color::DarkGray`, where it is text meant to be "secondary". Keep DarkGray where it is a drawn element that needs a fixed grey (for example the timeline's remainder, or the muted visualizer), and decide per use.
   - The selected row no longer uses `Color::Black` text on cyan: use reversed colours, or cyan with the terminal's background colour.
3. **CI** (`.github/workflows/ci.yml`):
   - **Triggers:** pushes to `main` and pull requests.
   - **Runners:** `ubuntu-latest` and `macos-latest`.
   - **Steps:** fmt check, both clippy variants, `cargo test --all-targets`, `cargo build`.
   - **Linux:** also install mpv and run `-- --ignored`, if it is reliable on runners.
   - **Hardening:**
     - least-privilege `permissions: contents: read`;
     - actions pinned by commit SHA (with the version in a comment);
     - a Rust cache;
     - no secrets.
4. **Release automation** (`.github/workflows/release.yml`):
   - **Trigger:** pushing a tag `vX.Y.Z`.
   - **Version check:** fail when the tag does not match `Cargo.toml`'s version.
   - **Build:** `coda` for Linux x86_64 and aarch64 (if practical without cross-compiling pain), and macOS arm64 and x86_64.
   - **Package:** tar.gz each binary with LICENSE and README, plus SHA-256 sums.
   - **Publish:** a GitHub release with `gh release create`, notes taken from that version's CHANGELOG section.
   - **Permissions:** `contents: write` on that job only.
   - **No crates.io publishing** (`publish = false` stays).
   - **How to release:** documented briefly in the README or a `docs/RELEASING.md`:
     1. bump the version;
     2. update the CHANGELOG;
     3. merge;
     4. tag;
     5. push the tag.
   - **Tagging:** the architect never creates or pushes the tag without the owner's explicit request.
5. **Full-codebase review** (read-only): dead code, stale comments, docs that no longer match the code across M1–M10, and test gaps. Findings are triaged by the architect; fixes are made in this slice when small.

## Out of scope
- Endless play and autoplay.
- Homebrew and crates.io.
- Signing or notarising macOS binaries. Document in the release notes or the README that macOS may quarantine a downloaded binary (`xattr -d com.apple.quarantine`).
