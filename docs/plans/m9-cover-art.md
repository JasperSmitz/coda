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
Briefed after the spike; see "Recommendation for slice 2" below.
