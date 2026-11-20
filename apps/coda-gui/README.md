# Coda App

A desktop music player for your local files and a personal [Navidrome](https://www.navidrome.org) server: the desktop frontend of [Coda](../../README.md), in the same repository as the terminal player. Built with Tauri 2, a Rust backend on Coda's shared engine (`crates/coda-core`), and a React frontend. Linux and macOS.

**Status:** early development, see [`docs/PLAN.md`](docs/PLAN.md).

## Finding your way

The bar at the top has **Home**, **Search** (`/` or Ctrl/Cmd+F) and **Library**, and the sliders button on the right opens **Preferences**. Each of the three keeps its own history: open an album from Library, switch to Home and back, and Library is still on that album; click the entry you are on to go back to its start. On a trackpad, a two-finger swipe to the left goes to the next of Home, Search and Library, to the right the previous one, one per swipe (not in a text box whose text runs past its edge, the queue, Now Playing, a dialog or a menu, or over something that scrolls sideways). Back is `Backspace`, `Alt+←`, the mouse's back button or Esc. **Library** shows your collection in sections, chosen at the top of the page: Albums, Artists, Genres, Playlists, Mixes, and Folders for a local library; the section you chose is remembered. Moving between pages crossfades, and a cover you open grows into the page's header cover (and back on Back); with the system's Reduce motion setting pages change at once. Pages scroll under the top bar and the player; on a long list the section control and the list's tools (sort, Shuffle, New playlist…) stay at hand in a bar that sticks under the top bar. An album, artist, genre or playlist page opens with a large cover, the title, **Play**, **Shuffle** and "…" (Add to playlist…, Add to mix…, Rename… and Delete… for your own Navidrome playlists). Albums, Artists, Genres, Playlists and a genre's albums show as **Covers** (tiles; hover one for its Play button, in the album's own colour) or as a **List** (compact rows; for albums with artist, genre, year and length, click a column head to sort); playlists as Covers are cards with a mosaic of their albums' covers; the icons in the tool bar switch, each section remembers its choice, and Preferences › Library sets the default. Albums sort A–Z, by artist, by year (local) or by **Most played**, and Navidrome also offers Recently added, Recently played and Random; the sort is remembered per source. Locally, Most played counts what you listened to in the Coda app (half a song or 4 minutes, like a scrobble; the terminal player does not count); Navidrome uses the server's play counts. An artist's page starts with **Your most played**: up to six of their songs you listened to most in the app, with the count; Enter or a double-click plays the album from that song. Shift+Enter on an album tile or row plays it.

**Home** is where you land. **Where you left off** shows the last albums, playlists and mixes you played (and artists or folders that have a cover), newest first, as a row of covers with the latest large in the middle; the ‹ › buttons beside the heading or a click on a smaller cover moves along, and once you have clicked a cover (or tabbed to it) ←/→ do too instead of seeking (elsewhere on Home they seek as usual); a click on the large one opens it. Under it: when you played it and where you stopped ("Played 2 hours ago · stopped in track 4 of 9"), **Resume**, which plays it again from that song at the point you left it (a mix just plays on; for what is playing now the button is **Now Playing**), and **Open album** (or playlist, mix). The page takes the colours of the large cover. **Moods for today** are three genres you listen to a lot, each with three of its albums you have not played for longest (never one from the last 7 days); they stay the same all day and change at midnight, and **Other moods** shows three more. **Play** on a card starts a quick mix of its three albums ("Quick mix: Jazz mood"), which plays on as long as you let it; a click on a cover opens that album. Before you have played anything, Home offers **Shuffle your library** and **Browse albums**, and the moods use your largest genres. Locally the genres come from the tags; on Navidrome from the server and the songs you played. `s` on Home shuffles the library.

**Search** has one large field ("Artists, albums, songs or lyrics"). Before you type, it shows the library's largest genres to jump to. Results start with the **Top result** (the artist or album that matches best) beside the first five **Songs** (Show all lists every matching song; Enter or a double-click plays them from that one), then **Albums**, **Artists** and **Lyrics**; ↓ in the field moves to the results. With both sources set up, "Search … instead" searches the other one. With both a music folder and a Navidrome server set up, you browse one at a time: choose it under **Music source** in Preferences, or press Ctrl/Cmd+1 (local) or Ctrl/Cmd+2 (Navidrome). The player floats at the bottom of the window: the cover and the track's names on the left, the controls and the timeline in the middle, a small visualizer, the volume, the queue and full screen on the right. `q` (or the player's queue button) opens the queue in a panel that rises out of the player; Esc, `q` or a click elsewhere closes it.

## The look and the accent

Coda 2.0 has one designed look: a dim, dark base with glass where things float, and the Fraunces and Figtree fonts (bundled with the app, under the SIL Open Font License). There are no colour themes and no light mode any more; 1.x's themes, Ghostty theme following and the colour editor are gone, and theme files you made in 1.x are left on disk untouched. What you can choose is the **accent** (Preferences › Accent). The default is **From the music**: a page takes the colours of the album it shows (an album, artist, genre or playlist page, and Home's large cover: the whole page in deep shades of the cover's main colours, with the cover's own art blooming out around it on Home, album and playlist pages, a soft glow behind the top elsewhere; Search, Library's lists and Preferences stay dark), and the player, the queue and Now Playing those of the song that's playing; colours blend when they change, and a black-and-white cover keeps Coda's amber. Or choose a fixed accent: Coda's own amber, one of five other colours, or any colour you type (`#rrggbb`) or pick; the pages and the player then keep their covers' dark tints but use your accent. The choice is saved in `~/.config/coda-app/settings.toml` and used from the next start's first frame; a colour that is hard to see on the dark background gets a note but is used as you chose it.

## Genres

**Genres** in Library shows the genres of the library you browse (Local or Navidrome), A–Z: as tiles tinted with the colours of each genre's first album, its cover peeking from the corner, or as a list with album and track counts (the Covers / List icons). Open one for its albums; **Play** or **Shuffle** (`s`) plays the whole genre (Navidrome: Shuffle plays random songs of the genre). An album's page shows its genres; click one to open it. Genres are read as tagged: a tag such as `Rock;Progressive Rock` counts as two genres (split on `;`, `/` and `,`, as Navidrome does), and Coda never changes or looks up genres.

## Mixes

A **mix** is a saved recipe of genres, artists and albums from one source, for example "Prog Rock and Indie Rock" or "King Crimson, Yes and Genesis". It plays as an endless shuffle in which the genres, artists and albums take turns, and it is resolved every time it plays, so music added later joins in. **Mixes** in Library lists the saved mixes of the source you browse: **Play** starts one, the card's menu (right-click or "…") edits or deletes it, and **New mix…** opens the editor, where you name the mix, search for genres, artists and albums and either **Save** it or **Play** it once without saving (a quick mix). **Add to mix…** in the menu of a genre, artist or album (and in the "…" menu on their pages) adds it to a saved mix or starts a new one. While a mix plays, Now Playing and the queue show "Mix: <name>"; playing anything else, or clearing the queue, ends it. An edit applies the next time the mix plays. Saved mixes live in `~/.local/share/coda/mixes.json`, shared with the terminal player.

## Now Playing and the window

`f` (or the Full screen button at the right of the player) opens Now Playing in full screen; closing it leaves full screen again, unless the window was already full screen before. A click on the player's cover opens Now Playing in a window, the cover growing into Now Playing's, and the button at its top right enters or leaves full screen without closing it. Coda always starts in a window, at the size and place it last had outside full screen (maximised if it was).

On macOS the window has no separate title bar: the close, minimise and zoom buttons sit at the left of the top bar (or over Now Playing), and you drag the window by the top bar or a page's header (double-click to zoom). Cmd+W, Cmd+M, Cmd+Q and the View and Window menus work as usual.

## Lyrics

In Now Playing (`f`), press `l` (or the lyrics button at the top right) for lyrics mode: the player moves to a column on the left and the lyrics fill the middle. Timed (LRC) lyrics follow the song and a click on a line seeks there; plain lyrics scroll. The mode stays on until you turn it off, and is off again after a restart. Lyrics come from the file's tags (`LYRICS`/`UNSYNCEDLYRICS`, MP4 `©lyr`, ID3 `USLT`) or a `.lrc` file with the same name next to it, and for Navidrome tracks from the server (timed with a recent Navidrome that offers `getLyricsBySongId`, plain text otherwise). Nothing is fetched from online lyrics services.

Search on Local also searches lyrics: a phrase from a song shows it under **Lyrics**, below the songs, albums and artists, with the matching line and the phrase marked (it plays like any track row). Case, curly and straight quotes and line breaks don't matter; type at least 3 characters. A song already listed under Songs is not repeated. The lyrics are read during the library scan (tags, else a `.lrc` next to the file) and kept in memory only.

Search on Navidrome searches lyrics too. Navidrome cannot search lyrics itself, so Coda builds an index of the server's lyrics on this computer (`~/.cache/coda/lyrics/`, one file per server and user, with no address or password in it): about 30 seconds after launch, and right after you connect a server, it asks the server for every album and song's lyrics in the background, a few requests at a time. The first time this takes a while for a large library (minutes for thousands of songs); while it runs, search shows the matches so far with a note. After that only new or changed albums are fetched at launch, and everything is checked again every 30 days. Preferences › Navidrome › Lyrics search shows the progress, has **Rebuild index**, and a switch **Index lyrics for search**; turning it off stops it and deletes the file (so does disconnecting the server). In memory it takes about 40 MB for 20,000 songs that all have lyrics.

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
