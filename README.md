# Coda

A keyboard-first terminal music player for local files and a personal Navidrome (Subsonic/OpenSubsonic) server, with one queue for both. Written in Rust; playback through mpv.

**Status:** alpha. Local playback (M1) and Navidrome (M2) work; see `docs/ROADMAP.md`.

## Requirements
- Linux or macOS
- Rust (stable)
- [mpv](https://mpv.io) 0.33 or newer, on `PATH`
- To store the Navidrome password from setup on Linux: a running Secret Service (GNOME Keyring or KWallet). Building needs no D-Bus headers or OpenSSL.

Install mpv:

```sh
# Arch Linux
sudo pacman -S mpv

# macOS (Homebrew)
brew install mpv
```

## Usage

```sh
coda
```

The first time, with no music source configured, Coda runs a short setup in the terminal: an optional local music folder (`~/Music` is suggested when it exists) and an optional Navidrome server (URL, username, password). It checks the folder, tests the connection, stores the password in the OS keychain, writes the config file below, and starts the player. Skipping both writes nothing; Coda asks again next time. If `config.toml` is a symlink (e.g. into a dotfiles repository), setup replaces the link with a regular file and leaves the link's target unchanged. Setup only runs when stdin and stdout are terminals; otherwise Coda prints the usage error as before.

A folder can also be given directly, which skips setup:

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
password_keychain = true   # the password is in the OS keychain (written by setup)
scrobble = true            # report plays to Navidrome (default)
# Or, instead of password_keychain (at most one of the three):
# password_command = ["pass", "show", "navidrome"]   # argv, no shell
# password = "..."                                    # only if the file is chmod 600
```

`password_keychain = true` reads the password from the macOS Keychain or, on Linux, the Secret Service (GNOME Keyring, KWallet; it must be running and unlocked), under the service `coda` and the account `<username>@<host>[:<port>]`. macOS may ask once whether Coda may read it. Without a keychain, use `password_command` or `password`; Coda never writes a plaintext password itself.

`CODA_NAVIDROME_URL`, `CODA_NAVIDROME_USER` and `CODA_NAVIDROME_PASSWORD` override the file. Coda uses salted-token auth and never logs URLs, tokens or passwords. With only Navidrome configured, Coda starts on the Navidrome library.

### Keys
`?` shows all keys. Highlights: `1`/`2` switch between the local and Navidrome libraries, `Enter` opens or plays now, `a` adds a track/folder/album/artist to the queue, `/` searches the active library, `Space` pauses, `n`/`p` skip, `←`/`→` seek, `+`/`-` volume, `Tab` switches panels, `q` quits. Remote tracks are marked `☁` in the queue. In the queue panel: `J`/`K` move, `s` shuffles upcoming, `g` jumps to the current entry, `C` (twice) clears; `R` cycles repeat off/all/one. The queue, volume and repeat mode are restored at the next launch (from `state.json` next to the log).

Logs are written to `$XDG_STATE_HOME/coda/coda.log` (default `~/.local/state/coda/coda.log`). Set `CODA_LOG=debug` for more detail.

## License
MIT
