# Coda

A keyboard-first terminal music player for your local files and a personal [Navidrome](https://www.navidrome.org) (Subsonic/OpenSubsonic) server, with one queue for both. Written in Rust; playback through [mpv](https://mpv.io).

**Status:** alpha, for personal use. Milestones M1–M7 are done; see [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Features
- **Play in context.** Enter plays an album or playlist from the selected track. `s` plays it shuffled, and that includes all of an artist's tracks. "Shuffle all" plays your whole library.
- **Local library:** browse by playlists (`.m3u`/`.m3u8`), folders, artists or albums. The scan runs in the background.
- **Navidrome:**
  - browse playlists, artists and albums (six sort orders);
  - stream into the same queue as local tracks;
  - scrobble plays;
  - create, rename, reorder and delete your playlists.
- **Setup:** a first run in the terminal, with the password stored in the macOS Keychain or the Linux Secret Service. You can connect or change Navidrome later without restarting.
- **Gapless playback:** consecutive tracks play without a pause, local files and Navidrome streams alike (the next stream is fetched ahead).
- **Session:** the queue, volume, repeat mode and visualizer are restored at the next launch.
- **Visualizer:** an optional spectrum and level meter, in step with what you hear.
- **Cover art:** the cover of the track you hear, in the now-playing bar: a real image in Ghostty and Kitty, colour blocks in other terminals. From the file's tags or its folder, or from Navidrome (cached on disk). See [Cover art](docs/USAGE.md#cover-art).
- **Now playing screen:** `f` swaps the lists for the cover with the title, the timeline and the visualizer beside it (or below it in narrow windows). See [Now playing screen](docs/USAGE.md#now-playing-screen).
- **Privacy:** logs never contain passwords, tokens, stream URLs, file paths or track titles.

## Requirements
- Linux or macOS (Windows is not supported)
- Rust (stable), to build
- [mpv](https://mpv.io) 0.33 or newer, on `PATH`
- To store a Navidrome password on Linux: a running Secret Service (GNOME Keyring or KWallet). Building needs no D-Bus headers or OpenSSL.

```sh
# Install mpv
sudo pacman -S mpv        # Arch Linux
sudo apt install mpv      # Debian/Ubuntu
brew install mpv          # macOS (Homebrew)

# Build and install Coda (puts `coda` in ~/.cargo/bin)
cargo install --path .
```

## Getting started

```sh
coda
```

On the first run, Coda asks for two optional sources:
1. **A local music folder.** `~/Music` is suggested when it exists.
2. **A Navidrome server:** its URL, your username and your password.

Coda tests the connection, stores the password in the OS keychain and saves the config. If you skip both, nothing is saved and Coda asks again next time.

| Command | What it does |
|---|---|
| `coda --setup` | Change the folder or the server later. |
| `O` (in the app) | Open Settings: connect, edit or disconnect Navidrome while music keeps playing. |
| `coda ~/Music` | Play a folder directly, without setup. `CODA_MUSIC_DIR=~/Music coda` does the same. |

## Keys
Press `?` in Coda for the full list.

| Key | Library | Queue |
|---|---|---|
| `j` `k` / `↓` `↑` | move | move |
| `l` `→` / `h` `←` `Backspace` | open / go back | — |
| `Enter` | play in context (replaces the queue) | play this entry |
| `s` | play shuffled (replaces the queue) | shuffle the upcoming entries |
| `a` / `A` | add to the queue / play next | — |
| `D` | remove from a playlist; on a playlist, delete it (asks y/n) | remove |
| `J` `K` | move a song in a playlist (`w` saves, `Esc` discards) | move |
| `/` | search | — |
| `o` / `R` | sort albums / reload (Navidrome) | — |
| `P` / `N` / `e` | add to a playlist / new playlist / rename | — |
| `S` / `g` / `C C` | — | save as a playlist / jump to current / clear |

| Key | Anywhere |
|---|---|
| `Space` | pause |
| `<` `>` | previous / next track |
| `,` `.` | seek 5 s back / forward |
| `+` `-` | volume |
| `r` | repeat: off, all, one |
| `v` | visualizer |
| `f` | Now playing screen (`f` or `Esc` goes back) |
| `1` `2` | local / Navidrome library (from the Now playing screen too) |
| `Tab` | switch between library and queue |
| `O` / `?` / `q` | settings / help / quit |

**How Enter works:**
- **On a track:** it plays the album, playlist, folder or search results the track is shown in, starting at that track. The earlier tracks come before it, so `<` goes back through them.
- **On an album or playlist:** it plays it from the start.
- **On an artist or folder:** it opens it.

The queue is temporary: `a` and `A` add to it, and a playlist is how you keep one. Navidrome tracks are marked `☁` in the queue.

## Configuration
Setup writes `~/.config/coda/config.toml` (or `$XDG_CONFIG_HOME/coda/config.toml`; `CODA_CONFIG` points elsewhere). You can also write it yourself:

```toml
music_dir = "~/Music"

[navidrome]
url = "https://music.example.org"
username = "me"
password_keychain = true   # the password is in the OS keychain (written by setup)
scrobble = true            # report plays to Navidrome (default)
# Or, instead of password_keychain (at most one of the three):
# password_command = ["pass", "show", "navidrome"]   # argv, no shell
# password = "..."                                    # only if the file is chmod 600

[ui]
cover_art = true           # cover art in the bar and on the Now playing screen (default); false turns it off
```

`CODA_NAVIDROME_URL`, `CODA_NAVIDROME_USER` and `CODA_NAVIDROME_PASSWORD` override the file.

**Files:**

| File | Where |
|---|---|
| Session (queue, volume, repeat, visualizer) | `$XDG_STATE_HOME/coda/state.json` |
| Log | `$XDG_STATE_HOME/coda/coda.log` |
| Navidrome covers (about 100 MB at most: 50 MB for the bar's, 50 MB for the Now playing screen's in `large/`; safe to delete) | `$XDG_CACHE_HOME/coda/covers/` |

`XDG_STATE_HOME` defaults to `~/.local/state`, `XDG_CACHE_HOME` to `~/.cache`. Set `CODA_LOG=debug` for more detail in the log.

**Backups:** when Coda replaces a config file it didn't write itself, it keeps the old one as `config.toml.bak.<time>`. **If that file held a plaintext `password`, so does the backup: delete it once you no longer need it.**

## More detail
- [`docs/USAGE.md`](docs/USAGE.md): the details of setup, settings, the keychain, playlist editing (including how Navidrome-hidden entries are handled), the local playlist rules, and limits.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): how Coda is built.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): what's done and what's next.

## License
MIT
