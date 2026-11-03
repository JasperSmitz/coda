# M9 — Cover art: brief

- **Status:** approved by owner 2026-11-02 ("more than ready for the cover art milestone").
- **Owner decisions:**
  - Cover art in the **now-playing bar only**.
  - Navidrome covers get a **size-capped disk cache**.
  - The owner runs **Ghostty** (Kitty graphics protocol) and **Alacritty** (no image protocol, so block characters).
- **Goal:** a square cover of the track you hear, at the left of the now-playing bar. It is a real image where the terminal supports one and calm block-character art elsewhere. It never slows input, playback or the visualizer, and it never shows anything for a track that isn't the one heard.
- **Delivery:**
  - **Slice 1** is a timeboxed feasibility spike with written findings.
  - **Slice 2** is the feature, briefed from the findings. It starts right away unless the spike finds something that needs the owner.
- **Out of scope:**
  - covers in the library list;
  - the full-screen "now playing" view (proposed as M10, see `docs/ROADMAP.md`);
  - artwork downloads from outside sources (only Navidrome and local files).

## Requirements (both slices)
- **Sources:**
  - **Navidrome:** `getCoverArt` with the song's or album's `coverArt` id, requested at a small size.
  - **Local:** the front cover embedded in the file (lofty, already a dependency), else an image file in the track's folder (`cover`, `folder`, `front`, `album`; `.jpg`, `.jpeg` or `.png`; case-insensitive).
- **Untrusted input:**
  - Image bytes come from the network and from files, so decode them with byte and pixel limits (no decompression bombs).
  - Decode and resize off the render loop (blocking threads).
  - A failing cover just shows none.
- **Privacy:**
  - Cover URLs carry auth tokens, so they are never logged.
  - Cache file names must not contain a token or URL: use a hash of server identity, account and `coverArt` id.
  - Logs carry no titles or paths, as now.
- **Disk cache (Navidrome only):**
  - location: `$XDG_CACHE_HOME/coda/covers` (default `~/.cache/coda/covers`), directory mode 0700;
  - what is stored: the resized images;
  - capped at about 50 MB, evicting least recently used;
  - a broken or unreadable cache entry is ignored and replaced.
  - Local covers are read from the files, with a small in-memory cache only.
- **Rendering:**
  - The cover follows the **audible** track (`Player::audible_load` / `playing_entry`, M8), so it switches when the next track is heard.
  - Fetches for a track that is no longer heard are dropped.
  - Nothing blocks the loop.
- **Fallback:** block characters (half blocks with colours) where no protocol is available. A config switch turns cover art off (`cover_art = false`, or similar, under a `[ui]` table).
- **Everything else keeps working:**
  - overlays (help, settings, prompts, the y/n question) drawn over the bar must not have an image showing through them;
  - resizing;
  - small terminals: the art hides itself, like the visualizer;
  - visualizer-only redraws (`Terminal::apply_buffer`);
  - terminal restoration on every exit path, with no images left on screen.

## Slice 1 — feasibility spike

### Questions
1. **Library.**
   - Does `ratatui-image` (or an alternative) work with our ratatui 0.30.2 and crossterm 0.29?
   - Which protocols does it support (Kitty, Sixel, iTerm2, half blocks), and how does it pick one?
   - What does it add to the build (the `image` crate features: JPEG and PNG only)?
   - Would a small hand-written Kitty-protocol encoder plus our own half-block renderer be simpler and safer?
2. **Detection.**
   - How is the protocol and the cell pixel size detected (terminal queries, environment variables)?
   - Can the queries run before the event reader starts, or with a timeout, so their replies never leak into keyboard input?
   - Results in Ghostty, Alacritty, macOS Terminal.app and inside `tmux` (passthrough off: block characters is fine).
   - What happens when detection fails or times out?
3. **Redraw interplay.**
   - The Kitty protocol places images outside the cell buffer. Does the image survive or flicker on ratatui's diffing redraws, on visualizer-only `apply_buffer` redraws at 30 Hz, and on a full clear after resize?
   - Can overlays cover it?
   - How are stale images deleted when the track changes, the art hides, or Coda exits (including a panic or a signal)?
4. **Look and cost.**
   - What size works in the now-playing bar (today 4 rows, plus 3 with the visualizer)? Should the bar grow when art is shown?
   - How good do half blocks look in Alacritty at that size?
   - CPU and time to decode and resize a typical 500–1500 px JPEG; memory per cached image.
5. **Navidrome API.**
   - Is `coverArt` present on songs and albums in `getAlbum`, `search3`, `getRandomSongs` and `getPlaylist`?
   - What do the `size` parameter and the response (content type, errors) look like?
   - Check against the Subsonic/OpenSubsonic docs and a wiremock fake; **never a real server**.

### Deliverable
- **Findings section.** A "Findings" section appended to this brief, with evidence for each question.
- **Recommendation.** End with a "Recommendation for slice 2":
  - library or hand-written;
  - the detection strategy;
  - layout and size;
  - the cache design;
  - how images are cleaned up;
  - the test plan.

  Flag anything needing the owner as **Owner decision needed**.
- **Throwaway code.** Prototypes live in the scratchpad or behind `#[ignore]` tests. The shipping app does not change in the spike.

## Slice 2 — cover art
**Architect decisions (2026-11-02), taken under the brief's "starts right away unless the spike finds something that needs the owner"; all are easy to revert:**
- **Bar height:** the now-playing bar grows to 6 rows when art is shown and the visualizer is off. With the visualizer on, the art uses the existing 5 rows. Without art (turned off, or no cover), the bar keeps today's height.
- **WebP:** decoding is included (pure Rust, about 0.36 MB), so Navidrome covers served as WebP show.
- **Navidrome's placeholder** (`Cache-Control: no-store`): show no art, and don't cache it.
- **Older session files without a cover id:** fall back to the song id. Navidrome serves song art by song id; on any other server the request fails and no art shows.

Implement "Recommendation for slice 2" below as written, plus these decisions.

**Acceptance:**
- **Manual:**
  - Ghostty shows the cover.
  - Alacritty and Terminal.app show half blocks, with no stray text after detection.
  - Inside tmux, Coda falls back to half blocks without touching tmux settings.
- **Automated:** every requirement above has a test (parser, encoder, byte-capture renders, stale results, cache, wiremock, PTY exit paths).
- **Unchanged:** `cover_art = false` restores today's layout exactly.

## Findings (slice 1 spike, 2026-11-02)

### Setup
- **Machine:** Apple M1 Pro, macOS, Rust via Homebrew cargo 1.99.1. No Coda source, test or manifest file was changed.
- **Terminals:**
  - **Ghostty 1.4.0** is installed and was driven with `open -na Ghostty --args -e <probe>`, with results written to files.
  - **Terminal.app** (`TERM_PROGRAM_VERSION` 488.9) was driven with `open -a Terminal <script>.command`.
  - **Alacritty and tmux are not installed.** Their rows below are reasoned from source and documentation, and are labelled as such.
  - **No screenshots:** `screencapture` fails in this sandbox ("could not create image from display"). Nothing was seen on screen. The evidence is terminal replies and the bytes written, captured through a PTY. Image quality was judged from an offline simulation of the cell grid (Q4).
- **Probes** (throwaway, in `scratchpad/m9/`):
  - `detect.py`, `detect2.py`: raw terminal queries, with timings and cursor reports.
  - `probe/`: a cargo project pinned to Coda's `Cargo.lock`, with `ratatui-image =11.1.0` and `image =0.25.10`. Its modes are:
    - `bench`: decode and resize timings, and decode limits;
    - `detect`: `ratatui-image`'s `Picker::from_query_stdio`, then crossterm events for 3 s to catch leaked replies;
    - `render`: Coda's redraw pattern. That is a full draw, 30 visualizer-only `apply_buffer` redraws at 30 Hz, an overlay, a `terminal.clear()` (as on resize), a track change and exit. It runs for three art renderers: `ratatui-image` Kitty, `ratatui-image` half blocks, and a hand-written Kitty encoder. Phases are marked in the output with an unknown OSC.
    - `preview`: draws half blocks and a real image at bar sizes into a PNG.
  - `ptycap.py`, `ptyscript.py`, `analyze.py`: the PTY harnesses (scripted terminal replies, late replies, keystrokes) and a byte counter for each phase.
  - `fake/`: a wiremock fake of `getCoverArt`, using Coda's reqwest settings (no redirects).
  - `nd/`: Navidrome source files read for Q5 (`master`, last change to `resize.go` on 2026-09-11; latest release v0.64.2).

### Q1: Library
- **Answer:**
  - `ratatui-image` 11.1.0 works with our ratatui 0.30.2 and crossterm 0.29: it builds against Coda's lock file with `default-features = false, features = ["crossterm"]`.
  - It has defects that matter to Coda (below), and it brings 34 extra crates for protocols we will not use.
  - **A hand-written Kitty encoder and half-block renderer, with `image` for decoding, is simpler and safer.**
- **`ratatui-image` 11.1.0:**
  - **Release and licence:** published 2026-09-17, MIT. Releases are frequent: 11.0.8 came out on 2026-09-04, and 12.0.0-rc.0 to rc.3 between 2026-09-17 and 2026-10-18, so the API keeps changing.
  - **Protocols:** Kitty (Unicode placeholders), Sixel (`icy_sixel`), iTerm2 and half blocks.
  - **How it picks one:** it queries the terminal (`ESC_Gi=31,…a=q…`, `CSI c`, `CSI 16t`, ending with `CSI 5n`) and parses the replies. In order of preference: Kitty, then Sixel from the DA1 reply, then iTerm2 from environment variables, then half blocks. Kitty and Sixel are blacklisted for WezTerm and Konsole.
  - **Default features must be turned off.** The defaults include `chafa-dyn` (pkg-config and a dynamically linked libchafa) and `image-defaults` (every `image` format, plus rayon).
- **Defects found in `ratatui-image`** (from source and probes):
  1. **A lost transmission.** The Kitty transmission is embedded in the symbol of the image's first cell and is marked as sent the first time the widget renders.
     - If an overlay covers that cell in the same frame, the transmission never reaches the terminal, and later frames draw placeholders for an image the terminal never received.
     - Probe evidence: `OVERLAY_FIRST=1 … render rimg-kitty` writes 0 `a=T` sequences in the first frame and in every later frame, until the track changes.
     - In Coda this happens whenever the track changes while help, settings, the picker or a prompt is open.
  2. **Startup stall and leaked replies.** Detection runs on a detached thread that reads stdin, with a 2 s timeout (PTY probe, `ptyscript.py`):
     - **A terminal that never answers:** a 2.0 s startup stall, and then the reader thread stays blocked on stdin forever, competing with crossterm for input.
     - **A reply that arrives late (2.5 s):** it leaks into crossterm as keys: `Alt-_`, `G`, `i`, `=`, `3`, `1`, `;`, `O`, `K`, `Alt-\`. In Coda's keymap `=` turns the volume up, `1` selects local, `K` moves a queue entry and `O` opens settings.
  3. **It changes tmux settings.** Inside tmux it runs `tmux set -p allow-passthrough on`, a subprocess that silently changes the user's pane setting.
  4. **Images are never deleted.** Each image gets a random id and no delete is ever sent, so images pile up in the terminal's image memory until the terminal's quota evicts them.
  5. **Duplicate dependency versions:** `rand` 0.8 (we have 0.10), `thiserror` 1 (we have 2) and `rustix` 0.38 (we have 1.1).
- **Build cost** (release builds of the probe, same lock file; Coda's release binary is 11.6 MB with 349 locked packages):

  | Variant | Binary | Crates in the normal dependency tree |
  |---|---|---|
  | ratatui + crossterm only | 0.83 MB | 63 |
  | + `image` 0.25.10 (`jpeg`, `png`), with decode, resize and encode used | 1.68 MB (+0.85) | 79 (+16) |
  | + `ratatui-image` 11.1.0 (no default features) | 2.06 MB (+0.37) | 113 (+34) |
  | `image` with `webp` as well (instead of the row above) | 2.05 MB (+0.36 over `jpeg`+`png`) | +1 (`image-webp`) |

- **What `image` adds:** with `jpeg` and `png`, the new crates are `zune-jpeg` 0.5.15, `zune-core`, `png` 0.18.1, `fdeflate`, `flate2`, `miniz_oxide`, `crc32fast`, `moxcms`, `pxfm`, `bytemuck`, `byteorder-lite`, `num-traits` and `simd-adler32`.
  - **Licences:** all are MIT or Apache-2.0 (zune-jpeg also allows Zlib).
  - **Age:** `image` 0.25.10 was published on 2026-03-10, so it is old enough to pin. Cargo also picks no pre-release zune versions.
- **What `ratatui-image` adds on top:** `icy_sixel`, `quantette`, `palette` (and its derive macros), `bitvec` (with `funty`, `radium`, `tap`, `wyz`), `wide`, `base64-simd`, `vsimd`, `outref`, `self_cell`, `ordered-float`, `ref-cast`, `rand_xoshiro`, `rand` 0.8, `thiserror` 1 and `rustix` 0.38.
- **The hand-written alternative** (`own_kitty` in `probe/src/main.rs`):
  - **Size:** about 70 lines: base64, chunked `a=T,U=1,q=2` transmission, delete, and placeholder cells.
  - **Behaviour:** it gave the same byte pattern as `ratatui-image` in every phase. It also deletes on exit.
  - **The rest of the estimate:**
    - half blocks: about 40 lines (the primitive renderer in `ratatui-image` is 60);
    - a gated query: about 120 lines, plus tests.
  - **What we give up:** Sixel and iTerm2. Neither of the owner's terminals uses them, and half blocks cover those terminals.

### Q2: Detection
- **Measured replies** (`detect2.py`: CPR, Kitty query, CPR, `CSI 16t`, `CSI 14t`, DA1):

  | Terminal | Kitty reply | Cell size | Pixel size from `TIOCGWINSZ` | DA1 | APC shown as text? | Round trip |
  |---|---|---|---|---|---|---|
  | Ghostty 1.4.0 | `_Gi=31;OK` | `16t` → 16×34 px | 792×592 for 49×17 cells (includes padding; dividing gives 16×34) | `?62;22;52c` | no (cursor stayed at column 1) | 0.3 ms |
  | Terminal.app 488.9 | none | no reply to `16t` (`14t` → 580×353) | 560×336 for 80×24 (7×14) | `?1;2c` | **yes: the cursor moved 31 columns** | 0.3 ms |
  | Alacritty (not installed; reasoned) | none (no graphics protocol) | — | — | answers DA1 (from memory of its source, not run) | no: its parser crate `vte` (0.15.0 source checked, `SosPmApcString` state) ignores APC | — |
  | tmux, passthrough off (not installed; from documentation) | none (tmux does not forward unknown APC) | — | — | tmux answers itself | no | — |

- **More Ghostty results:**
  - It accepted a transmission with a virtual placement (`a=T,U=1,f=32,i=4242,…`, not quiet) and replied `_Gi=4242;OK`. It accepted a delete (`a=d,d=I,i=4242`) without error.
  - Ghostty's Unicode placeholder support is known from memory of its release notes, not checked in this spike. **That the image actually shows was not seen** (no screenshots); this is a manual smoke check for slice 2.
- **`ratatui-image` detection** inside real terminals (`probe detect`):
  - **Ghostty:** Kitty with a 16×34 font, in 0.42 ms, with nothing leaked (only two `Resize` events from the window opening).
  - **Terminal.app:** half blocks with 7×14, in 0.41 ms, with nothing leaked.
- **Environment variables are not reliable on their own.** The agent's shell inside Ghostty passed `TERM_PROGRAM=ghostty` to a PTY child that was not Ghostty. An Alacritty window opened from a Ghostty shell would inherit it in the same way, since Alacritty does not set `TERM_PROGRAM`. `TERM` is set by each terminal and survives ssh, but users often override it.
- **Can the query run before the event reader?** Yes, and it must.
  - **The reply must be fully drained before `EventStream` starts.** Anything after that is parsed as keys (see the late-reply result above).
  - **End the query with DA1 (`CSI c`).** Every terminal tested answers DA1, so its reply marks the end of the answers.
  - **Read on the thread that will run the loop**, with `poll(2)` and a deadline. No detached reader thread.
- **What happens when detection fails or times out:** use half blocks. A reply that arrives after the deadline can still leak as keys, so the query must be rare and the deadline generous. See the strategy in the recommendation.
- **Terminal.app prints the Kitty APC as visible text.** Ratatui's first frame does not rewrite cells that are blank in both buffers. So either skip the query there, or clear the screen after detection (both are recommended).

### Q3: Redraw interplay
- **Kitty with Unicode placeholders keeps the image in the cell buffer.**
  - The image is transmitted once, with a virtual placement (`U=1`).
  - Each cell then holds `U+10EEEE`, plus row and column diacritics, with the image id encoded in the foreground colour.
  - So ratatui's diffing, overlays and clears treat the image like text.
- **Bytes written in each phase** (PTY capture at 100×30, 8×4-cell art, hand-written Kitty encoder; `ratatui-image` Kitty is the same except where noted):

  | Phase | Kitty APC sequences | Placeholder cells written | Notes |
  |---|---|---|---|
  | First full draw | 9 (one transmission in 9 chunks, about 35 KB) | 32 | |
  | 30 visualizer-only `apply_buffer` redraws | 0 | 0 | 8.2 KB in total, all visualizer cells |
  | Second full draw | 0 | 0 (1 for `ratatui-image`: the first cell's symbol no longer carries the transmission) | |
  | Overlay opened over the art | 0 | 0 | the overlay's text replaces the placeholder cells, so no image shows through |
  | Overlay closed | 0 | 24 (the cells that were covered) | |
  | `terminal.clear()` (resize) | 0 | 32, after `CSI 2J` | no new transmission is needed: the terminal keeps the image by id |
  | Track change | `ratatui-image`: one new transmission with a new random id; the old image is never deleted | 28–32 | |
  | Exit | hand-written: `a=d,d=I,i=<id>` then `CSI ?1049l`; `ratatui-image`: nothing | 0 | |

- **Flicker:** none expected. The visualizer path never touches the art cells, and full draws re-emit only the cells that changed.
- **Overlays:** they hide the image cell by cell, because an overlay's cells replace the placeholders. Half blocks are ordinary cells, so they behave like text too.
- **Resize:** with `c=`/`r=` set, the terminal scales the image to the placement's cells. A font-size change therefore needs no new transmission. A different art size (for example the visualizer turned on) needs a new placement, or a new transmission.
- **Cleaning up stale images:**
  - **Track change or hidden art:** stop drawing the placeholders, and send `a=d,d=I,i=<old id>` to free the image's memory.
  - **Normal exit:** the same delete before leaving the alternate screen.
  - **Panic and signals:** the panic hook and the signal paths already leave the alternate screen. The placeholders are text on the alternate screen, so they disappear with it, and nothing can remain on the main screen. The hook can also write the delete, ignoring errors.
  - Ghostty's per-screen image storage on exit was not observed directly.

### Q4: Look and cost
- **Square art at bar heights** (Ghostty cells are 16×34 px, about 1:2.1, so a square is about twice as many columns as rows):

  | Art rows | Columns (Kitty: rows × 34/16; half blocks: rows × 2) | Kitty pixels | Half-block "pixels" |
  |---|---|---|---|
  | 2 (today's bar without the visualizer: inner height 2) | 4 | about 64×68 | 4×4 |
  | 4 | 8–9 | about 136×136 | 8×8 |
  | 5 (today's bar with the visualizer: inner height 5) | 10–11 | about 170×170 | 10×10 |

- **How half blocks look:** judged from `scratchpad/m9/preview-parrot.png`, a simulation with real photos at these sizes. They show the cover's colour palette and its rough layout (a red blob on green), not the picture. At 2 rows they are a 4×4 swatch. The result is a calm colour mosaic, which suits Alacritty as ambience. They need truecolor: Alacritty supports it, and Terminal.app 488.9 advertises `COLORTERM=truecolor`.
- **How a Kitty image looks:** at 2 rows (64 px) it is recognisable but small. At 4–5 rows it is good.
- **CPU** (`probe bench`, M1 Pro, one thread, release build, synthetic photo-like JPEGs; time to resize to 256 px):

  | Source | JPEG size | Decode | Triangle | `thumbnail` | Lanczos3 |
  |---|---|---|---|---|---|
  | 500 px | 79 KB | 3.0 ms | 4.3 ms | 3.8 ms | 7.6 ms |
  | 1000 px | 293 KB | 6.5 ms | 4.8 ms | 3.7 ms | 10.7 ms |
  | 1500 px | 664 KB | 11.2 ms | 7.5 ms | 5.9 ms | 19.8 ms |
  | 3000 px | 2.6 MB | 43.6 ms | 26.7 ms | 17.6 ms | 71.2 ms |

  - Encoding the 256 px result costs 0.4–1.4 ms (PNG or JPEG).
  - So about 20 ms per typical cover, on a blocking thread.
- **Memory:**
  - **While decoding:** up to width × height × 3 transiently (1500 px: 6.75 MB).
  - **A kept 256 px RGBA image:** 256 KB.
  - **One Kitty transmission** of a 170×176 RGBA image: about 120 KB raw, 160 KB base64. With RGB (`f=24`) it is three quarters of that.
- **Decode limits work** (`image::Limits`, `max_alloc` 64 MiB, 4096×4096 maximum):
  - a 739-byte JPEG whose header claims 65000×65000 is rejected in 7 µs ("Memory limit exceeded"), even with only `max_alloc` set;
  - a real 5000 px JPEG is rejected in 0.6 ms;
  - `ImageReader::decode` reserves the output size before decoding (image 0.25.10, `image_reader_type.rs`).
- **Local embedded art:** Coda's scan already reads with `read_cover_art(false)`. A cover read uses `read_cover_art(true)`, bounded by lofty's global `allocation_limit` (default 16 MiB, lofty 0.25.4 `global_options.rs`).

### Q5: Navidrome API
- **`coverArt` on songs and albums:**
  - In OpenSubsonic, `coverArt` is an optional string on both `Child` and `AlbumID3` (open-subsonic-api `Responses/Child.md`, `AlbumID3.md`).
  - All four endpoints return it:
    - `getAlbum`: on the album and on each song;
    - `search3`: on songs and albums;
    - `getRandomSongs` and `getPlaylist`: on each `Child` entry (`getPlaylist` also has one for the playlist).
  - Navidrome fills it with ids such as `mf-<id>_<hash>` and `al-<id>_<hash>`, as in Coda's own fixture in `tests/navidrome_client.rs`. The hash changes when the art changes, so the id works as a cache key.
  - **Coda does not parse `coverArt` today**, and the session file keeps only `song_id`.
- **`getCoverArt` parameters:** `id` (required) and `size` (optional, "scale image to this size"). OpenSubsonic: "`id` refers to coverArt ID only".
  - Navidrome also accepts a raw entity id: its `parseArtworkID` falls back to `GetEntityByID`. That is not portable to other servers.
- **Navidrome's behaviour** (master, `core/artwork/{artwork,resize}.go`, `server/subsonic/media_retrieval.go`, `server/imghttp/headers.go`):
  - **Size:** `size` is clamped to the original. An original already no larger than `size` is sent **unchanged, in any format** (it could be GIF or WebP).
  - **Format of a resized image:** JPEG (quality 75), or PNG when the original is PNG or `square=true`. With `EnableWebPEncoding` (off by default) it is WebP.
  - **Content-Type:** it is not set explicitly, so Go detects it from the content.
  - **Unknown id:** a Subsonic error 70 ("Artwork not found"). With `f=json` it is a JSON body with HTTP 200; the OpenSubsonic docs say XML with `text/xml`.
  - **An entity without art:** Navidrome sends a **placeholder image** (a generic album or artist picture) with `Cache-Control: no-store`. Real art carries an `ETag`, and `immutable` when the id's hash matches.
  - **Timeout:** the server gives up after 10 s.
- **The wiremock fake** (`scratchpad/m9/fake`, reqwest 0.13.5, redirects off; magic bytes sniffed before trusting `Content-Type`):

  | Fake response | Classified as |
  |---|---|
  | JPEG with HTTP 200 | image |
  | Subsonic JSON error with HTTP 200 | error |
  | 3 MB body against a 2 MB cap | too big, stopped while streaming chunks |
  | 302 | not followed |
  | 404 | HTTP error |

### Open risks
- Ghostty's display of Unicode placeholders was not seen on screen (no screen capture here).
- Alacritty and tmux behaviour was reasoned from source and documentation, not run.
- A Kitty-capable terminal reached over a slow link (ssh) could answer after the query's deadline, and its reply would leak as keys. Gating and a generous deadline make this rare but not impossible.
- One Kitty transmission (about 120–160 KB) is written synchronously by the loop on each cover change. That is negligible locally, and tens of milliseconds over slow ssh.

## Recommendation for slice 2

- **Library: hand-written, with `image` for decoding.**
  - Add `image = { version = "=0.25.10", default-features = false, features = ["jpeg", "png"] }`. Do not add `ratatui-image`.
  - Write a small `src/ui/cover.rs` (or similar) with:
    - a Kitty encoder: chunked `a=T,U=1,q=2,f=24` transmissions, `a=d,d=I` deletes, and placeholder cells with id-coloured foregrounds and the diacritics table;
    - a half-block renderer;
    - a detection parser.
  - Keep decoding, resizing and the disk cache in a module that does not depend on the UI (for example `src/cover/`), returning plain RGB images. Rendering turns them into cells.
  - **Owner decision needed (minor): WebP.** Navidrome may send WebP: originals smaller than the requested size, or servers with `EnableWebPEncoding`. Adding the `webp` feature costs about 0.36 MB and one pure-Rust crate. Without it, those covers show none. Recommendation: include it.
- **Detection strategy:**
  1. **When to query:** only when `[ui] cover_art` is on, `$TMUX` is unset, `TERM` does not start with `screen` or `tmux`, and the environment names a Kitty-capable terminal: `TERM` is `xterm-kitty` or `xterm-ghostty`, `TERM_PROGRAM=ghostty`, or `KITTY_WINDOW_ID` is set. In every other case, use half blocks with no query; this covers Alacritty, Terminal.app, tmux and WezTerm. WezTerm's placeholder support is incomplete, as `ratatui-image` notes.
  2. **How to query:** after `ratatui::try_init()` and before creating `EventStream`, write the Kitty query, `CSI 16t` and DA1. Read the replies on the same thread with `poll(2)`, until the DA1 reply, with a deadline of 1 s (Ghostty answers in 0.3 ms).
  3. **The result:** Kitty only if the Kitty reply is `OK`. The cell size comes from `16t`, else from `TIOCGWINSZ` (pixels divided by cells), else 1:2.
  4. **Afterwards:** `terminal.clear()`, so no stray query text survives.
  5. **On timeout:** half blocks, and a `debug` log line.
  - Parse the replies with a small parser that is unit-tested on Ghostty's exact bytes, on replies split across reads, and on garbage.
- **Layout and size:**
  - The art is a square at the left edge of the bar's inner area, spanning its full inner height. The info, progress and visualizer rows move right by the art's width plus one column.
  - Columns are rows × cell height ÷ cell width for Kitty, and rows × 2 for half blocks.
  - The art hides (with no space reserved) when the screen is narrower than about 60 columns, or lower than `VISUALIZER_MIN_SCREEN.1`, or when nothing is playing or there is no cover.
  - **Owner decision needed: the bar grows when art is shown.** Today's 4-row bar leaves 2 rows for art (4×4 half blocks, about 64 px with Kitty). Recommendation: when art is shown and the visualizer is off, grow the bar to 6 rows (inner rows: title, artist · album, blank, progress). With the visualizer on, the bar stays 7 rows and the art uses all 5 inner rows.
- **Fetching and state:**
  - Add the cover id to remote tracks (`coverArt` from the four endpoints), and persist it in the session as an optional field, so files stay version 1 (as before).
  - **Architect decision needed:** restored tracks from older session files have no cover id. Either show no art for them, or fall back to the song id. The fallback works on Navidrome but is not OpenSubsonic-portable.
  - The cover follows `Player::playing_entry` / `audible_load`. Each request is keyed by the cover's key (Navidrome: server, account and `coverArt`; local: the path). A result applies only if its key matches the audible entry's key when it arrives. Otherwise it goes to the small in-memory cache and is not shown. One request is in flight per key.
  - **HTTP:** `getCoverArt?id=…&size=256` through the existing client, with signed URLs that are never logged and `.without_url()` on errors.
    - The body is capped at 4 MB and streamed in chunks.
    - Classify by magic bytes: JPEG, PNG, WebP if enabled, anything else an error.
  - **Owner decision needed (minor): Navidrome's placeholder image.** A response with `Cache-Control: no-store` is Navidrome's placeholder image, or provisional art. Recommendation: do not cache it, and show no art (calmer than a generic icon).
  - **Decoding:** `spawn_blocking`, with `Limits` of `max_alloc` 64 MiB and 4096×4096, then `thumbnail` to at most 256 px. Any error shows no art and logs only the kind of error at `debug`.
  - **Local covers:** read the front cover from the tags (lofty `read_cover_art(true)`, `PictureType::CoverFront`, else the first picture). Otherwise use the first `cover`, `folder`, `front` or `album` file (`.jpg`, `.jpeg` or `.png`, case-insensitive) in the track's folder that is a regular file of at most 16 MB. Keep an in-memory LRU of about 16 decoded 256 px images (4 MB).
- **Disk cache design (Navidrome only):**
  - **Location:** `$XDG_CACHE_HOME/coda/covers` (default `~/.cache/coda/covers`), created with mode 0700.
  - **File name:** hex of a hash of (base URL, username, `coverArt`) plus `.jpg`. The `md5` crate is already a dependency; this naming is not a security boundary, but sha2 would do if the reviewer prefers. No token, salt or URL is ever part of a name.
  - **Contents:** the decoded image resized to at most 256 px, saved as a JPEG of about 85 % quality (roughly 10–30 KB, so 50 MB holds a few thousand covers). It is written to a `tempfile` in the same folder and renamed into place.
  - **LRU without an index:** a hit updates the file's modification time. When a write takes the total over 50 MB, delete the oldest files until it is under 45 MB. The scan is bounded (for example 20 000 entries) and runs on a blocking thread.
  - **Broken entries:** an entry that fails to decode is deleted and fetched again.
- **Cleaning up images:**
  - **Kitty ids:** a counter starting from a random 24-bit base; there are few ids per session.
  - **On a track change, or when the art hides or is turned off:** send `a=d,d=I,i=<old id>` through the backend before the next draw.
  - **On normal exit (including SIGTERM and SIGHUP):** delete before `try_restore`, ignoring errors (on a hung-up tty the write fails harmlessly).
  - **In the panic hook:** write the same delete, ignoring errors, before restoring.
  - **Overlays:** they need no special handling, because they replace the placeholder cells. Because the transmission is sent separately from the cells (before `terminal.draw`), an overlay can never lose it.
- **Test plan:**
  - **Unit tests:**
    - the detection parser (Ghostty's bytes, Terminal.app's, no Kitty reply, replies split across reads, garbage, timeout via a pipe or PTY);
    - environment gating (`TMUX`, `screen*`, inherited `TERM_PROGRAM` with a different `TERM`);
    - the Kitty encoder (chunk size and `m=` flags, `q=2`, base64 against known vectors, delete format);
    - placeholder cells (diacritics and colour id);
    - half-block colours;
    - layout sizes (hide thresholds, a 6-row bar with the visualizer off);
    - decode limits (header claiming 65000², truncated JPEG, a non-image, WebP).
  - **Render tests with a byte-capturing backend:**
    - a visualizer-only redraw writes no art cells;
    - an overlay replaces every placeholder it covers;
    - a full redraw after `clear` re-emits placeholders without a new transmission;
    - a track change writes a delete for the old id and one transmission for the new one;
    - a track change under an open overlay still transmits.
  - **App tests:**
    - a result for a track that is no longer audible is not shown;
    - a gapless switch changes the art to the next track's;
    - one request is in flight per key;
    - `cover_art = false` makes no queries or fetches.
  - **Cache tests:** mode 0700; file names without a token or URL; LRU eviction at the cap; a corrupt entry replaced; `no-store` not cached.
  - **wiremock tests:** an image; a JSON error with HTTP 200; 404; a redirect not followed; a body over the cap; the URL absent from logs and errors.
  - **Local tests:** an embedded front cover preferred; folder file names case-insensitive; a file over the cap ignored.
  - **PTY smoke tests:** exiting with `q`, SIGTERM, SIGHUP and a panic each write the delete and leave the alternate screen; no leaked keys with a terminal that replies late.
  - **Manual checks:**
    - **Ghostty:** art visible; overlays cover it; resize and font-size change; the visualizer running at 30 Hz with no flicker.
    - **Alacritty:** half blocks.
    - **Terminal.app:** half blocks, no stray text.
    - **tmux in Ghostty:** half blocks; the pane's settings unchanged.
