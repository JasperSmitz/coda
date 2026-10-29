# Coda

A keyboard-first terminal music player for local files and a personal Navidrome (Subsonic/OpenSubsonic) server, with one queue for both. Written in Rust; playback through mpv.

**Status:** alpha. Local playback (M1) and Navidrome (M2) work; see `docs/ROADMAP.md`.

## Requirements
- Linux or macOS
- Rust (stable)
- [mpv](https://mpv.io) 0.33 or newer, on `PATH`

Install mpv:

```sh
# Arch Linux
sudo pacman -S mpv

# macOS (Homebrew)
brew install mpv
```

## Usage

```sh
coda ~/Music
# or
CODA_MUSIC_DIR=~/Music coda
```

### Configuration (optional)
`~/.config/coda/config.toml` (or `$XDG_CONFIG_HOME/coda/config.toml`; `CODA_CONFIG` points elsewhere):

```toml
music_dir = "~/Music"

[navidrome]
url = "https://music.example.org"
username = "me"
password_command = ["pass", "show", "navidrome"]   # argv, no shell
# password = "..."   # only if the file is chmod 600
```

`CODA_NAVIDROME_URL`, `CODA_NAVIDROME_USER` and `CODA_NAVIDROME_PASSWORD` override the file. Coda uses salted-token auth and never logs URLs, tokens or passwords. With only Navidrome configured, Coda starts on the Navidrome library.

### Keys
`?` shows all keys. Highlights: `1`/`2` switch between the local and Navidrome libraries, `Enter` opens or plays now, `a` adds a track/folder/album/artist to the queue, `/` searches Navidrome, `Space` pauses, `n`/`p` skip, `←`/`→` seek, `+`/`-` volume, `Tab` switches panels, `q` quits. Remote tracks are marked `☁` in the queue.

Logs are written to `$XDG_STATE_HOME/coda/coda.log` (default `~/.local/state/coda/coda.log`). Set `CODA_LOG=debug` for more detail.

## License
MIT
