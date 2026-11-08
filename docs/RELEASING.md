# Releasing Coda

A release is made by pushing a version tag. The `Release` workflow (`.github/workflows/release.yml`) then:
1. checks that the tag is `v` followed by the version in `Cargo.toml`, and that `CHANGELOG.md` has a section for that version;
2. builds `coda` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` and `x86_64-apple-darwin`;
3. packages each as `coda-<version>-<target>.tar.gz` (the binary, `LICENSE`, `README.md`) with a `.sha256` file;
4. creates the GitHub release with that CHANGELOG section as its notes. A version with a suffix (`1.1.0-rc.1`) becomes a pre-release.

Nothing is published to crates.io (`publish = false`).

## Steps
1. **Bump the version** in `crates/coda-tui/Cargo.toml`, then run `cargo build -p coda-tui` so `Cargo.lock` follows (the workflow builds with `--locked`).
2. **Update `CHANGELOG.md`:** move the entries from `[Unreleased]` into a new `## [X.Y.Z] - YYYY-MM-DD` section, written for users, and update the links at the bottom.
3. **Merge** to `main` through a pull request, with CI green.
4. **Tag** the merge commit on `main`:
   ```sh
   git switch main && git pull
   git tag -a vX.Y.Z -m "Coda X.Y.Z"
   ```
5. **Push the tag:** `git push origin vX.Y.Z`. Follow the run under the repository's Actions tab.

If the run fails before the release is created, fix the cause on `main`, then move the tag: `git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`, and tag and push again. If a release was created, delete it on GitHub first.

## Notes
- The macOS binaries are not signed or notarised; the release notes and the README say how to clear the quarantine flag.
- The Intel macOS binary is built on the Apple silicon runner (`--target x86_64-apple-darwin`) and is not run in the workflow.
- The Linux binaries are built on Ubuntu 24.04 and link against its glibc (2.39).
