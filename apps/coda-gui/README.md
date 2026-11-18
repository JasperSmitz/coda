# Coda App

A desktop music player for your local files and a personal [Navidrome](https://www.navidrome.org) server: the desktop frontend of [Coda](../../README.md), in the same repository as the terminal player. Built with Tauri 2, a Rust backend on Coda's shared engine (`crates/coda-core`), and a React frontend. Linux and macOS.

**Status:** early development, see [`docs/PLAN.md`](docs/PLAN.md).

## Genres

**Genres** in the sidebar lists the genres of the library you browse (Local or Navidrome), A–Z with their album and track counts. Open one for its albums; **Play** or **Shuffle** (`s`) plays the whole genre (Navidrome: Shuffle plays random songs of the genre). An album's page shows its genres; click one to open it. Genres are read as tagged: a tag such as `Rock;Progressive Rock` counts as two genres (split on `;`, `/` and `,`, as Navidrome does), and Coda never changes or looks up genres.

## Mixes

A **mix** is a saved recipe of genres, artists and albums from one source, for example "Prog Rock and Indie Rock" or "King Crimson, Yes and Genesis". It plays as an endless shuffle in which the genres, artists and albums take turns, and it is resolved every time it plays, so music added later joins in. **Mixes** in the sidebar lists the saved mixes of the source you browse: **Play** starts one, the card's menu (right-click or "…") edits or deletes it, and **New mix…** opens the editor, where you name the mix, search for genres, artists and albums and either **Save** it or **Play** it once without saving (a quick mix). **Add to mix…** in the menu of a genre, artist or album (and on their pages) adds it to a saved mix or starts a new one. While a mix plays, Now Playing and the queue show "Mix: <name>"; playing anything else, or clearing the queue, ends it. An edit applies the next time the mix plays. Saved mixes live in `~/.local/share/coda/mixes.json`, shared with the terminal player.

## Now Playing and the window

`f` (or the Full screen button at the right of the bottom bar) opens Now Playing in full screen; closing it leaves full screen again, unless the window was already full screen before. A click on the bar's cover opens Now Playing in a window, and the button at its top right enters or leaves full screen without closing it. Coda always starts in a window, at the size and place it last had outside full screen (maximised if it was).

On macOS the window has no separate title bar: the close, minimise and zoom buttons sit at the top of the sidebar (or over Now Playing), and you drag the window by a page's header, the queue's header or the top of the sidebar (double-click to zoom). Cmd+W, Cmd+M, Cmd+Q and the View and Window menus work as usual.

## Lyrics

In Now Playing (`f`), press `l` (or the lyrics button at the top right) for lyrics mode: the player moves to a column on the left and the lyrics fill the middle. Timed (LRC) lyrics follow the song and a click on a line seeks there; plain lyrics scroll. The mode stays on until you turn it off, and is off again after a restart. Lyrics come from the file's tags (`LYRICS`/`UNSYNCEDLYRICS`, MP4 `©lyr`, ID3 `USLT`) or a `.lrc` file with the same name next to it, and for Navidrome tracks from the server (timed with a recent Navidrome that offers `getLyricsBySongId`, plain text otherwise). Nothing is fetched from online lyrics services.

Search on Local also searches lyrics: a phrase from a song shows it under **Lyrics**, below the artists, albums and tracks, with the matching line and the phrase marked (it plays like any track row). Case, curly and straight quotes and line breaks don't matter; type at least 3 characters. A song already listed under Tracks is not repeated. The lyrics are read during the library scan (tags, else a `.lrc` next to the file) and kept in memory only.

Search on Navidrome searches lyrics too. Navidrome cannot search lyrics itself, so Coda builds an index of the server's lyrics on this computer (`~/.cache/coda/lyrics/`, one file per server and user, with no address or password in it): about 30 seconds after launch, and right after you connect a server, it asks the server for every album and song's lyrics in the background, a few requests at a time. The first time this takes a while for a large library (minutes for thousands of songs); while it runs, search shows the matches so far with a note. After that only new or changed albums are fetched at launch, and everything is checked again every 30 days. Settings › Navidrome › Lyrics search shows the progress, has **Rebuild index**, and a switch **Index lyrics for search**; turning it off stops it and deletes the file (so does disconnecting the server). In memory it takes about 40 MB for 20,000 songs that all have lyrics.

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
