# Coda — Usage details

The README covers everyday use. This page collects the details: edge cases, limits, and what Coda does with your files and credentials.

## Setup (`coda` on first run, `coda --setup`)

### First run
- **When it runs:** only when no music source is configured and both stdin and stdout are terminals. Without a terminal, Coda prints the usage error instead.
- **The two sources:** setup asks for an optional local folder, then an optional Navidrome server.
- **Before saving:** it checks the folder, tests the connection and stores the password in the OS keychain. Only then does it write the config file and start the player.
- **Skipping both** writes nothing, and setup runs again next time.
- **Leaving with `Ctrl-C`** changes nothing.
- **A symlinked config:** if `config.toml` is a symlink (for example into a dotfiles repository), setup replaces the link with a regular file and leaves the link's target alone.

### `coda --setup`
- **Starting point:** the current config. The folder, URL and username are filled in. The password is asked again, because Coda never reads the old one for this. A login embedded in the saved URL is never shown.
- **Removing a source:** skipping the folder or Navidrome removes it from the config, and Navidrome's stored password with it. At least one of the two must stay.
- **Settings made outside Coda** are pointed out where they apply:
  - `CODA_MUSIC_DIR` or a folder argument, which is checked before setup starts;
  - the `CODA_NAVIDROME_*` variables, which keep overriding the file;
  - a hand-written `password` or `password_command`, which saving replaces with the keychain.
- **A partly overridden server:** while `CODA_NAVIDROME_URL` or `CODA_NAVIDROME_USER` is set without `CODA_NAVIDROME_PASSWORD`, setup cannot save a server, because Coda would look the password up under the account those variables name. It offers to keep `[navidrome]` as it is, or to remove it; in that case, unset the variables too.
- **A changed account:** the old keychain entry is deleted after the new config is written.
- **When setup refuses to start:**
  - a config file that does not parse, or has unsafe permissions, must be fixed by hand first;
  - `--setup` needs a terminal. Without one it exits with status 2.

## Settings in the app (`O`)
Settings shows the music folder and the Navidrome status:
- connected as user @ host;
- not configured;
- the last connection error;
- configured outside Coda, naming the variable or config key.

The keys:

| Key | What it does |
|---|---|
| `c` | Connects when Navidrome is not configured. |
| `e` | Edits a server Coda set up. |
| `d d` | Disconnects. |

**Connecting and editing:**
- **The form** is the same URL, username and password form as setup. The password is asked again.
- **Before saving:** the server is tested with a connection.
- **What is saved where:** the password goes to the keychain and `[navidrome]` to the config file. The music folder and `scrobble` in the file are kept.
- **A replaced server:** the old keychain entry is deleted once the new config is written.

**Disconnecting:**
- It removes `[navidrome]` and the stored password.
- It is refused unless the config file keeps a music folder. A folder given only as an argument or in `CODA_MUSIC_DIR` does not count.

**Servers configured outside Coda** (`CODA_NAVIDROME_*`, or a hand-written `password` or `password_command`) are shown but not changed. Connecting is refused while the variables only partly override the file, for the same reason as in setup.

**Other notes:**
- **Without Navidrome,** pressing `2` offers "Press Enter to connect", which opens the same form.
- **Playback during a change:** the track playing keeps playing. Queued Navidrome tracks use the new server from their next load. After a disconnect, they are skipped like any track that cannot be played.
- **Quitting during a save:** the first `Ctrl-C` only warns. A second one quits once the save has finished, waiting at most 5 seconds.
- **The music folder** is changed with `coda --setup`, not here.

## The config file and its backups
- **Backups:** a file with settings is kept as `config.toml.bak.<time>` before it is replaced. This doesn't happen when the file is exactly as Coda wrote it, so repeated saves do not pile up backups.
- **What carries over:** only `scrobble`, `[ui]`, and a kept `[navidrome]`.
- **A plaintext password in the backup:** if the old file held a plaintext `password`, the backup still contains it. Delete the backup once you no longer need it.
- **Permissions:** Coda writes the config with mode 0600 and its folder with mode 0700.

## Keychain
- **Where the password is read from:** `password_keychain = true` reads it from the macOS Keychain or, on Linux, the Secret Service (GNOME Keyring or KWallet; it must be running and unlocked).
- **The entry:** service `coda`, account `<username>@<host>[:<port>]`.
- **The first read:** macOS may ask once whether Coda may read the entry.
- **A missing or unreadable entry:** Coda exits with a message that suggests `coda --setup`.
- **Without a keychain:** use `password_command` (an argv list, run without a shell) or `password` (only with the file at mode 600). Coda never writes a plaintext password itself.
- **Authentication and logs:** Coda authenticates with salted tokens and never logs URLs, tokens or passwords.

## Playing
- **The queue cap:** the queue holds up to 10,000 entries. A longer context is cut around the selected track: first before it, then at the end.
- **Adding while something loads:** while a Navidrome list for Enter or `s` is still loading, what you add with `a` or `A` waits and is added after it starts.
  - Adds still loading from before are dropped, since the new queue replaces them; the status line says so.
  - `C C` cancels a play that is still loading.
- **Play next:** `A` adds right after the current track. Several `A`s in a row keep their order.
- **Shuffle all:**
  - **Local:** every track in a random order. With more than 10,000 tracks, a random 10,000. While the scan is still running, or after it failed, it uses the tracks scanned so far, and the status line says so.
  - **Navidrome:** 500 random songs from the server, a new set on every press. Pressing again while a set is loading waits for that set.
- **Stereo:** Coda always plays in stereo. Mono is played on both channels, and mpv downmixes surround.
- **Gapless playback:** the next track in the queue starts without a pause, also under repeat. Coda gives mpv a copy of the next track in advance, and mpv opens it (for Navidrome: starts downloading it) while the current one plays.
  - **A change of sample rate** between two tracks (for example 44.1 to 48 kHz) still leaves a pause of about 70 ms while the audio output reopens. The tracks of an album normally share one rate.
  - **AAC (`.m4a`)** joins can be off by about 20 ms; this comes from mpv and FFmpeg.
  - **A queue change in the last moment of a track** (about the last quarter second, longer on Bluetooth) is corrected: the track you removed or replaced does not play, at the cost of a slightly cut ending and a short pause.
  - **Navidrome** sees the next song's stream request early, once the current one has been fetched. A song you then skip costs one wasted request. Scrobbling is unaffected.
- **The visualizer:**
  - It is off by default; `v` turns it on.
  - It shows frequency bands and a left/right level meter in the now-playing bar while a track plays.
  - It needs a terminal of at least 20x16.
  - From 42 columns wide, the bars spread across the width.
- **The session file:** the queue, volume, repeat mode and visualizer setting are saved in `state.json` and restored at the next launch.

## Cover art
The cover of the track you hear is shown at the left of the now-playing bar. It changes when the next track is heard, so with gapless playback it switches at the join, not when the next track is opened. While the next track's cover loads, the previous one stays (at most a second), so the tracks of an album show their cover without a blink.

- **Where it comes from:**
  - **Local tracks:** the front cover embedded in the file (else its first picture), else an image in the track's folder named `cover`, `folder`, `front` or `album` with `.jpg`, `.jpeg` or `.png` (any case, in that order of preference).
  - **Navidrome tracks:** the server's cover for the song (or its album), asked for at 256 pixels. Navidrome's grey placeholder for songs without art is not shown.
  - Nothing is downloaded from anywhere else.
- **How it looks:**
  - **Ghostty and Kitty:** a real image. Coda asks the terminal once at startup (it answers in well under a millisecond; Coda waits at most one second) and clears the screen afterwards.
  - **Every other terminal** (Alacritty, Terminal.app, WezTerm, iTerm2, and anything inside tmux or screen): a mosaic of coloured half blocks, two pixels per cell. These terminals are not asked anything. Inside tmux Coda never changes tmux settings.
  - **Colours:** half blocks use 24-bit colour when `COLORTERM` is `truecolor` or `24bit` (Alacritty and Terminal.app set it), else the nearest of the 256 standard colours.
- **The layout:** with art, the now-playing bar grows from 4 to 6 rows (title, artist · album, progress). With the visualizer on, the art sits beside it in the bar's existing 7 rows. A track without a cover keeps the usual bar.
- **When it hides:** on screens narrower than 60 columns or lower than 16 rows, and when nothing is playing. Help, Settings and the other boxes are drawn over it; the image never shows through them.
- **Loading** never holds up keys, playback or the visualizer: covers are read and decoded in the background, and a cover that arrives after you moved on is not shown. Images larger than 4096 pixels a side, or files over 16 MB (4 MB from Navidrome), are ignored.
- **The cache:** Navidrome covers are kept, resized to 256 pixels, in `$XDG_CACHE_HOME/coda/covers` (default `~/.cache/coda/covers`, a private folder). The cache holds at most about 50 MB; the least recently shown covers are removed first. File names are hashes: they contain no server address, username or token. You can delete the folder at any time. The folder is only created when the first Navidrome cover is stored. Local covers are only kept in memory (the last 16). Embedded covers are not read from files over 256 MB; their folder image is used instead.
- **Turning it off:** put this in the config file. The bar is then exactly as without cover art, and the terminal is not asked anything.

  ```toml
  [ui]
  cover_art = false
  ```
- **If the image looks wrong** (for example after changing the font size in Ghostty), restart Coda; it measures the cell size once at startup.

## Local library
- **The home menu:** Shuffle all, Playlists, Folders, Artists and Albums.
- **Grouping by tags:** artists and albums are grouped by the album artist, or by the artist when there is none. `o` in Albums switches between A–Z and by artist.
- **Album rows** show the track count, and the running time when every track's length is known.
- **Playlists** are the `.m3u`/`.m3u8` files in the music folder. A playlist lists the scanned tracks it names, in its order.
  - **Paths:** relative to the playlist, or absolute inside the music folder. Windows `\` separators work.
  - **Counted as missing:** URLs, drive letters, and anything outside the music folder.
  - **Exact matching:**
    - Names are case-sensitive and not Unicode-normalised, so an NFD name in a playlist does not match an NFC file name.
    - Absolute entries must spell the music folder's real path, with symlinks resolved. On macOS that means `/private/tmp`, not `/tmp`.

## Navidrome library
- **The home menu:** Shuffle all, Playlists, Artists and Albums.
- **Albums** load page by page as you scroll. Album rows show the year and the running time.
  - **`o` changes the order:** A–Z, by artist, newest, most played, recently played, or 500 at random.
  - **`R` reloads,** or retries a page that failed.
- **Search (`/`)** finds artists, albums and songs.

### Editing playlists
Only your own playlists can be edited; other users' playlists are read-only.

| Where | Key | What it does |
|---|---|---|
| queue panel | `S` | Saves the queue's Navidrome songs as a new playlist. Local tracks are left out. The name defaults to `Queue <today's date>`. |
| on a song, album, artist or playlist | `P` | Adds it to a playlist you pick, or to a new one. |
| Playlists list | `N` | Creates an empty playlist. |
| Playlists list | `e` | Renames the playlist. |
| Playlists list | `D` | Deletes the playlist. It asks first; only `y` deletes and any other key cancels. |
| inside a playlist | `D` | Removes the selected song. |
| inside a playlist | `J` `K` | Move the selected song. The title shows "(modified)". |
| inside a playlist | `w` / `Esc` | Saves the new order / discards it. |

An empty name cancels.

**Changes are checked before they are saved.** Before removing a song or saving a new order, Coda reloads the playlist. If it changed on the server in the meantime, nothing is changed and the new version is shown.

**Entries Navidrome hides are dropped.** Removing a song or saving a new order rewrites the playlist from the songs Coda shows. Entries Navidrome hides are therefore removed from the playlist: songs whose files are missing, and songs in libraries you cannot access.
- **The warning:** when Navidrome's song count reveals such entries, Coda warns first ("Press D again …: this also removes 3 entries Navidrome hides …"). It changes the playlist only on a second `D` or `w`; any other key cancels.
- **No warning:** a server that does not report a song count gives none.

**Other limits:**
- **The last song** of a playlist cannot be removed; delete the playlist instead.
- **A failed multi-request change:** if a change needed several requests and failed part-way, the status line says how much was done.
- **Quitting during a save:** while a change is being saved, the first `q` only warns. Quitting then waits up to 5 seconds for the save, and changes not yet started are dropped.
- **Queue and playlists are separate:** editing the queue never changes a playlist, and editing a playlist never changes the queue.
