# Brief: platform — Linux build verification, packaging, CI

Read first: `AGENTS.md`, `docs/ARCHITECTURE.md`, `README.md`. The owner's main desktop is **Arch Linux**; development so far happened on macOS and **nothing has been compiled for Linux yet** (souvlaki/zbus MPRIS, `PR_SET_PDEATHSIG` in `crates/coda-core/src/playback/mpv/process.rs`, tauri-plugin-dialog's GTK backend, window-state).

Docker is available locally (Linux, aarch64). Nothing may be pushed or published.

## 1. Linux build + tests in Docker
- `docker/linux-dev.Dockerfile`: a Debian/Ubuntu (or `archlinux` if an arm64 image works; otherwise Debian trixie/Ubuntu 24.04 is fine) image with Rust stable, Node 22 + pnpm, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libssl-dev`, `patchelf`, `xdg-utils`, `file`, mpv. Cache cargo registry/target in named volumes so rebuilds are fast; mount the repo read-write but use a separate `CARGO_TARGET_DIR` inside the container so macOS and Linux targets never mix.
- `scripts/linux-check.sh`: builds the image if needed and runs in the container: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, the ignored mpv tests (`-- --ignored`, mpv is installed; use `--ao=null` paths as the tests do), and `pnpm install --frozen-lockfile && pnpm build`, then `pnpm tauri build --debug --no-bundle`.
- Fix whatever fails on Linux (cfg-gated code, missing deps, warnings). Keep fixes minimal and platform-correct; don't break macOS (re-run the macOS checks too).

## 2. Packaging
- `tauri.conf.json` bundle config: Linux targets `deb`, `appimage`, `rpm` optional; macOS `dmg`/`app`. Proper product name "Coda", identifier, category `Audio`/`AudioVideo`, short/long description, desktop entry (MimeType not needed), the declared dependency on `mpv` for deb (`depends`), and app icons: generate a simple, tasteful placeholder icon set (`pnpm tauri icon` from a 1024px source SVG/PNG you create — a minimal mark, not Tauri's logo).
- `packaging/arch/PKGBUILD` that builds from a local checkout/tag (depends: `webkit2gtk-4.1`, `gtk3`, `mpv`, `libayatana-appindicator` if needed; makedepends: `rust`, `cargo`, `nodejs`, `pnpm`), installs the binary, `.desktop` file and icons. Test it in Docker if an Arch image is usable on arm64; otherwise validate by building the deb/AppImage in the Debian container and document that the PKGBUILD is untested.
- README: install/build instructions for Arch and macOS (mpv requirement, `xattr` quarantine note for unsigned macOS builds like Coda's README).

## 3. CI (file only; it will run once the owner pushes)
`.github/workflows/ci.yml` mirroring Coda's style (pinned action SHAs as in `../coda/.github/workflows/ci.yml`, read-only permissions, concurrency): fmt, clippy, tests on ubuntu-latest (with the webkit deps) and macos-latest, plus `pnpm build`. Optionally a release workflow building deb/AppImage/dmg on tags (mirroring `../coda/.github/workflows/release.yml`).

## Acceptance
Linux container: all checks pass, and `tauri build --debug --no-bundle` produces a Linux binary; a release bundle (`deb` and/or AppImage) builds in the container. macOS checks still pass. Report what was run and its results. Don't commit; report to `/tmp/coda-app-platform-report.md`.
