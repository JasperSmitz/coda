# Coda App

A desktop music player for your local files and a personal [Navidrome](https://www.navidrome.org) server: the desktop frontend of [Coda](../../README.md), in the same repository as the terminal player. Built with Tauri 2, a Rust backend on Coda's shared engine (`crates/coda-core`), and a React frontend. Linux and macOS.

**Status:** early development, see [`docs/PLAN.md`](docs/PLAN.md).

## Requirements

Coda plays through [mpv](https://mpv.io), which must be installed (it is looked for on `PATH`, then in `/opt/homebrew/bin`, `/usr/local/bin` and `/usr/bin`). The Navidrome password is kept in the OS credential store: the macOS Keychain, or on Linux a Secret Service provider such as GNOME Keyring or KWallet.

## Install

### Arch Linux

Build a package from a checkout with the included PKGBUILD. It builds the committed tag `coda-gui-v<version>` of the repository it sits in (set `_ref=branch=main` or `_ref=commit=<sha>` for something else):

```sh
sudo pacman -S --needed base-devel git rust nodejs pnpm   # rustup works instead of rust
cd packaging/arch
makepkg -si          # installs coda-app; pulls in mpv and webkit2gtk-4.1
```

This installs `/usr/bin/coda-app`, a desktop entry ("Coda") and icons.

### Debian, Ubuntu, Fedora, other Linux

Build the bundles (see Development for the build dependencies):

```sh
pnpm install
pnpm tauri build --bundles deb,appimage,rpm -- --profile release-gui
# ../../target/release-gui/bundle/deb/Coda_<version>_<arch>.deb (depends on mpv), .../appimage/, .../rpm/
```

### macOS

```sh
brew install mpv
pnpm install
pnpm tauri build --bundles app,dmg -- --profile release-gui   # ../../target/release-gui/bundle/macos/Coda.app and dmg/Coda_<version>_aarch64.dmg
```

The app is not signed or notarised. A `Coda.app` copied from a dmg downloaded in a browser is quarantined and macOS says it "is damaged" or "can't be verified"; remove the flag once: `xattr -dr com.apple.quarantine /Applications/Coda.app`. Alternatively, open it once, then choose **Open Anyway** in System Settings → Privacy & Security. A build made on your own Mac is not quarantined.

## Development

Requirements: Rust (stable), Node 22+, pnpm, mpv, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/). On Arch: `webkit2gtk-4.1`, `base-devel`, `curl`, `wget`, `file`, `openssl`, `librsvg`. On Debian/Ubuntu: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libssl-dev patchelf file`.

```sh
pnpm install
pnpm tauri dev
```

Checks (the Rust build embeds `dist/`, so build the frontend first; the cargo commands cover the whole workspace, TUI included):

```sh
pnpm build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                 # add `-- --ignored` for the tests that start mpv
```

`scripts/linux-check.sh` runs these for `coda-app` and `coda-core` (not the TUI), plus `pnpm tauri build --debug --no-bundle`, in a Debian container (`docker/linux-dev.Dockerfile`), with the Linux target dir, cargo registry and `node_modules` in Docker volumes so they never mix with the host's. `scripts/linux-check.sh --bundle` also builds the release deb, AppImage and rpm; `--shell` opens a shell in the container.

The icon set comes from `src-tauri/icons/source/icon.svg`: `pnpm tauri icon src-tauri/icons/source/icon.svg` (then delete the generated `android/` and `ios/` folders).
