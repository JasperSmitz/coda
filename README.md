# Coda

A keyboard-first terminal music player for local files and, later, a personal Navidrome server. Written in Rust.

**Status:** pre-alpha. Nothing is playable yet; see `docs/ROADMAP.md`.

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

## Usage (planned for M1)

```sh
coda ~/Music
# or
CODA_MUSIC_DIR=~/Music coda
```

Logs are written to `$XDG_STATE_HOME/coda/coda.log` (default `~/.local/state/coda/coda.log`). Set `CODA_LOG=debug` for more detail.

## License
MIT
