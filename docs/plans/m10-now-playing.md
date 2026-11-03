# M10 — Now playing screen: brief

- **Status:** approved by owner 2026-11-03 ("Go ahead! Sounds good.").
- **Owner decisions:**
  - `f` toggles the screen.
  - Layout, top to bottom:
    - album · artist;
    - a large central cover;
    - the title in **bold**, with a blank line around it (terminals cannot make text bigger);
    - the visualizer;
    - the timeline.
  - A larger cover is fetched for this screen: Navidrome at about 1000 px, cached apart from the 256 px covers. Local covers are decoded at that size too.
  - Extras appear only when they say something:
    - **Next up** shows in the last 20 s of a track. It also shows for a few seconds after a queue change alters what comes next.
    - **Volume** shows for about 2 s after `+` or `-`.
    - **Repeat** is a small dim `⟳` or `⟳1`, shown only while repeat is on.
    - **Paused** dims the timeline and the visualizer, with no added text.
  - Everything except the cover, the title and the timeline is dim grey.
- **Goal:** a calm display for listening, built on M9. It never slows input, playback or the visualizer, and it never shows anything for a track other than the one heard.
- **Delivery:** one slice. Run no spike unless something below turns out not to work. In that case, stop and report.
- **Out of scope:**
  - lyrics, ratings, other metadata;
  - controls beyond the existing global keys;
  - endless play and autoplay (a separate future feature);
  - persisting the screen across launches. Coda starts in the library as now.

## Requirements

### Screen and keys
- `f` switches between the library/queue view and the Now playing screen, from either panel. On the screen, `f` and `Esc` go back.
- **Global keys keep working on the screen:** `Space`, `<` `>`, `,` `.`, `+` `-`, `r`, `v`, `?`, `O`, `q`, `1` `2`.
  - **Exception, `1` and `2`:** these return to the library view first, then switch source.
- **Keys that only make sense in the library or queue** (`j` `k`, `Enter`, `a`, `D`, `/`, …) do nothing on the screen. They must not act on a list you can't see.
- **Help and docs:** `?` lists `f`. Update the keys tables in README and `docs/USAGE.md`, and USAGE gets a short "Now playing screen" section.
- **Overlays** (help, settings, any prompt or y/n question that can still open) keep the M9 rule: no image shows through them.
- **The bottom status row** still shows messages and errors.
- **Nothing playing:** the screen shows a dim "Nothing playing" in the centre, with the timeline area empty.

### Layout
- **Album · artist:**
  - one dim line, centred, truncated with `…`;
  - when the album is missing, the artist alone; when both are missing, nothing.
- **Cover:**
  - the largest square-looking area that fits between the text lines and the visualizer, centred;
  - Kitty graphics where detected, half blocks elsewhere (M9's `ArtPainter`).
  - **No cover** (none found, `cover_art = false`, or a terminal too small): the cover's room goes to the visualizer and the text is centred vertically. The screen must still look intentional.
- **Title:**
  - bold, centred, with a blank line above and below;
  - long titles are truncated with `…` (no marquee).
- **Visualizer:**
  - wider and taller than in the bar: it uses the screen's width with sensible side margins and several rows (for example 6–10, scaled with height);
  - follows `v`; when it is off, the cover takes the room.
  - The visualizer-only redraw must keep working for this larger area.
  - Raising `MAX_BARS` or the bar height is fine if `visualizer_layout` and `band_curve` scale cleanly. Keep "the vibe" (option b from M7).
- **Timeline:**
  - elapsed, a progress bar, and the duration, centred and bounded in width (not edge to edge on a very wide window);
  - the repeat indicator and the transient volume sit beside it, dim.
- **Next up:**
  - one dim line under the timeline: `Next: <title> — <artist>`, truncated.
  - **It names the entry that will actually play next:** with repeat all, the first entry after the last; with repeat one, or when nothing follows, it never shows.
  - **When it shows:**
    - in the last 20 s of the current track;
    - for about 4 s when a queue change, or a skip, alters what comes next.
  - Tracks shorter than 20 s simply show it throughout.
- **Paused:** the timeline and the visualizer are dimmed.
- **Small terminals:**
  - with too little room, the cover goes first, then the visualizer;
  - the text and timeline always fit, down to the existing minimum screen size;
  - pick and test concrete thresholds.
- **Redraws:** transient extras (volume, Next up) appearing and disappearing must trigger redraws via the existing tick and dirty logic, not by redrawing every frame.

### Large cover
- **Size variant:** load and cache a second size for the Now playing screen.
  - **Navidrome:** `getCoverArt` with `size` about 1000, same byte cap or a justified larger one.
  - **Disk cache:** a separate cache (for example `covers/large/`, or a size-tagged name) with its own cap of about 50 MB and LRU eviction. Check `MAX_ENTRY_BYTES` against real 1000 px JPEGs. The README files table must say the total is about 100 MB.
  - **Local:** decoded at the large size within M9's decode limits.
  - **Fetching:** only while the screen is open (or opening). The bar keeps using 256 px.
  - **In memory:** keep only a couple of large images (they're about 3 MB each decoded).
- **While loading:** the 256 px cover is shown upscaled until the large one arrives, never a blank. The M9 linger rules still apply to track changes.
- **Kitty encoder limits:**
  - `DIACRITICS` holds only 32 entries, which caps an image at 32 cells. Use the full table from Kitty's `rowcolumn-diacritics.txt` (297 entries) and test the bounds.
  - **Transmitted pixels:** raise `MAX_TRANSMIT_PIXELS` for the large screen, to roughly 1024 or the area's pixel size, whichever is smaller.
  - **Send cost:** measure the bytes sent for one transmission. If plain RGB gets large, consider the protocol's zlib compression (`o=z`), only if it is simple and tested.
  - **Re-sending:** no re-send unless the picture or the cell size changes. Toggling the screen or resizing re-sends at the new size, and the old image is deleted.
- **Cleanup:** every exit path still deletes the image, as in M9, including the panic hook.

### Safety and privacy
- Same as M9: untrusted bytes, decode limits, no tokens or URLs in logs or cache names, nothing blocking the loop.

## Verification
- **Unit tests:**
  - layout at several sizes (with and without cover, visualizer on/off, too small);
  - Next up visibility (timing, repeat modes, end of queue, short tracks, queue change);
  - transient volume;
  - key routing on the screen;
  - the large-variant request, the cache and the fallback;
  - the diacritics table bounds;
  - transmission size.
- **The checks in AGENTS.md:** fmt, both clippy variants, tests, `--ignored` with mpv.
- **pty smoke runs**, extended for `f`, with scripted replies and a hard timeout. **Never open a terminal emulator or any GUI app, never write to `/dev/tty`** (AGENTS.md, LESSONS).
- **On-screen checks** are listed for the owner to run in Ghostty and Alacritty. The developer does not do them.
