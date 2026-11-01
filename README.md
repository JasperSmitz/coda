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

To change the folder or server later, run the setup again:

```sh
coda --setup
```

It starts from the current config file: the folder, URL and username are filled in and the password is asked again (Coda never reads the old one for this). A login embedded in the saved URL is never shown. Skipping the folder or Navidrome removes it from the config, and Navidrome's stored password with it; at least one of the two must stay. Settings made outside Coda are pointed out where they apply: `CODA_MUSIC_DIR` or a folder argument (checked before setup starts), the `CODA_NAVIDROME_*` variables (which keep overriding the file), and a hand-written `password` or `password_command`, which saving replaces with the keychain. While `CODA_NAVIDROME_URL` or `CODA_NAVIDROME_USER` is set without `CODA_NAVIDROME_PASSWORD`, setup cannot save a server (Coda would look its password up under the account those variables name); it offers keeping `[navidrome]` as it is, or removing it (then unset the variables too). If the account changes, the old keychain entry is deleted after the new config is written. Leaving with `Ctrl-C` changes nothing. While settings are being saved in the app, the first `Ctrl-C` only warns; a second one quits once the save has finished (waiting at most 5 seconds). A config file that does not parse, or has unsafe permissions, must be fixed by hand first. `--setup` needs a terminal; without one it exits with status 2.

Navidrome can also be connected, changed or removed while Coda runs, without stopping playback. Press `,` for Settings: it shows the music folder and the Navidrome status (connected as user @ host, not configured, the last connection error, or configured outside Coda, naming the variable or config key). `c` connects when Navidrome is not configured, and `e` edits a server Coda set up: the same URL, username and password form as setup (the password is asked again), tested with a connection before anything is saved. The password goes to the keychain and `[navidrome]` to the config file; the music folder and `scrobble` in the file are kept, and the old keychain entry is deleted once the new config is written. `d` (twice) disconnects: it removes `[navidrome]` and the stored password, and is refused unless the config file keeps a music folder (a folder given only as an argument or in `CODA_MUSIC_DIR` does not count). A server configured outside Coda (`CODA_NAVIDROME_*`, or a hand-written `password` or `password_command`) is shown but not changed; while `CODA_NAVIDROME_URL` or `CODA_NAVIDROME_USER` is set without `CODA_NAVIDROME_PASSWORD`, connecting is refused for the same reason as in setup. Pressing `2` without Navidrome offers "Press Enter to connect", which opens the same form. The track playing keeps playing; queued Navidrome tracks use the new server from their next load (after a disconnect they are skipped like any track that cannot be played). The music folder is changed with `coda --setup`. `,` is a provisional key.

A file with settings is kept as `config.toml.bak.<time>` before it is replaced (not when it is exactly as Coda wrote it, so repeated saves do not pile up backups); only `scrobble` (and a kept `[navidrome]`) is carried over. **If the old file held a plaintext `password`, the backup still contains it: delete the backup once you no longer need it.**

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

`password_keychain = true` reads the password from the macOS Keychain or, on Linux, the Secret Service (GNOME Keyring, KWallet; it must be running and unlocked), under the service `coda` and the account `<username>@<host>[:<port>]`. macOS may ask once whether Coda may read it. If the entry is missing or the keychain cannot be read, Coda exits with a message suggesting `coda --setup`. Without a keychain, use `password_command` or `password`; Coda never writes a plaintext password itself.

`CODA_NAVIDROME_URL`, `CODA_NAVIDROME_USER` and `CODA_NAVIDROME_PASSWORD` override the file. Coda uses salted-token auth and never logs URLs, tokens or passwords. With only Navidrome configured, Coda starts on the Navidrome library.

### Keys
`?` shows all keys. Highlights: `1`/`2` switch between the local and Navidrome libraries, `Enter` opens or plays now, `a` adds a track/folder/album/playlist/artist to the queue, `A` adds it to play next (right after the current track; several `A`s in a row keep their order), `/` searches the active library, `Space` pauses, `n`/`p` skip, `←`/`→` seek, `+`/`-` volume, `Tab` switches panels, `,` opens Settings, `q` quits. Remote tracks are marked `☁` in the queue. The Navidrome library opens on a home menu: Playlists, Artists and Albums (`h` goes back to it). Albums load page by page as you scroll; `o` changes their order (A–Z, by artist, newest, most played, recently played, or 500 at random) and `r` reloads (or retries a page that failed). In the queue panel: `J`/`K` move, `s` shuffles upcoming, `g` jumps to the current entry, `C` (twice) clears; `R` cycles repeat off/all/one. The queue, volume and repeat mode are restored at the next launch (from `state.json` next to the log).

Logs are written to `$XDG_STATE_HOME/coda/coda.log` (default `~/.local/state/coda/coda.log`). Set `CODA_LOG=debug` for more detail.

## License
MIT
