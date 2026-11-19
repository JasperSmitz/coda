# Coda App — Design (2.0, "Neo-Coda")

**Status: target design for 2.0, being built in slices (`PLAN.md`, "2.0").** Until a slice lands, the code still shows the 1.x design, which is kept at the tag `coda-gui-v1.0.0` (`git show coda-gui-v1.0.0:apps/coda-gui/docs/DESIGN.md`). Each section below says *(shipped)* once its slice is merged; everything else is the agreed target. The direction and the retired theme system: ADR 0009. The reference mockup (invented music, generated covers): https://claude.ai/artifact/VpXdkrF5jpKBoNJ8va2Ez3 (owner-only).

## Principles
- **A native app, not a web page.** No link styling, no hover underlines, text not selectable except in inputs, the default cursor on controls, right-click context menus where they help. Views scroll on their own; the chrome floats over them.
- **Designed, not configurable.** One opinionated look. Personalisation is the accent colour (the music's, or one fixed colour), not themes.
- **Colour belongs to what it is attached to.** A page takes the colours of what it shows, the player those of what plays; everything else rests on a calm, dim base. Colours blend slowly; nothing flashes.
- **Roomy by default, dense on request.** Big covers and air; every long collection has a list view.
- **Calm motion with one moment at a time.** Things glide and settle; nothing bounces for attention. At most one decorative flourish in the whole app.
- **The music is the decoration.** Covers, their colours and the visualizer carry the look; the chrome stays quiet glass.

## Layout

```text
┌──────────────────────────────────────────────────────────────────────┐
│ ● ● ●  𝄌 coda          ( Home  Search  Library )            [⚙]      │  floating, glass
│                                                                      │
│   page (scrolls under the bar and the player)                        │
│   ░░░ the page's glow: colours of what it shows, fading out below ░░ │
│                                                                      │
│        ┌─────────────────────────────────┐  queue rises out of       │
│        │ Queue                           │  the player (q)           │
│        └─────────────────────────────────┘                           │
│   ╭──────────────────────────────────────────────────────────────╮   │
│   │ ▣ title / artist      ⏮ ⏯ ⏭  0:42 ━━━━━━━ 4:10   ▂▄▆ 🔊 ☰ ⤢ │   │  floating player
│   ╰──────────────────────────────────────────────────────────────╯   │
└──────────────────────────────────────────────────────────────────────┘
```

- **Top bar** (no background of its own; a soft fade of the base colour under it so the page reads as passing beneath): the coda sign and wordmark on the left (on macOS right of the window buttons), the main navigation centred, Preferences on the right. The bar drags the window (double-click zooms); its buttons work as usual.
- **Main navigation**: a glass pill with three entries, **Home**, **Search**, **Library** (icon + label; icons only in narrow windows). The active entry has a lighter pill that glides between entries.
- **Window (macOS)**: no title bar; the native window buttons sit at the top-left in the top bar's row, vertically centred on it (hidden by macOS in full screen). Linux keeps the window manager's title bar; the top bar then starts at the top of the content.
- **Player**: a floating rounded strip (~76px tall, ≤1000px wide, centred, 16px above the bottom edge). See Player.
- **Page**: the full window width under the bar, content ≤1240px centred, padded so nothing hides under the bar or the player at the ends of the scroll.
- Minimum window 900×600 (unchanged). No sidebar and no queue column.

## Navigation
- Three roots: **Home**, **Search**, **Library**. Each keeps its own back stack (Back: `Backspace`, `Alt+←`, the mouse's back button, Esc when nothing is open, or the page's ‹ Back): Library → Artist → Album, Home → Album, …
- **Library** holds the collection as sections in a segmented control at the top: **Albums, Artists, Genres, Playlists, Mixes**, and **Folders** for a local library. The chosen section is remembered.
- **Preferences** (the slider icon, top right) is a page of its own, outside the three roots.
- **One source at a time** stays, but leaves the main UI: the source is chosen in Preferences (and with `Ctrl/Cmd+1`/`2`). With one source configured there is nothing to choose. The choice is saved in `~/.config/coda-app/settings.toml` as today (default: Navidrome if configured, else Local; a source that goes away falls back to the other). Search's "Search … instead" link and "Go to album/artist" on a track of the other source (switches first) stay. Back stacks are per source, as in 1.x.
- **Now Playing** covers everything (`f`, the player's expand button, or a click on the player's cover).

## Home
The landing page. Two things, and room around them.

1. **Where you left off** (heading, with the day and part of the day above it in the small italic label style, e.g. "Thursday evening"). A carousel of the ~8 most recent listening contexts (albums first; playlists and mixes as their cover or mosaic), newest first:
   - The focused cover is large (~30% of the page width, 180–330px), centred; its neighbours smaller (×0.72, then ×0.6) and dimmed, fading out by the fourth. ←/→ (when Home has focus and nothing else takes the keys), the ‹ › buttons, a horizontal trackpad swipe, or a click on a neighbour moves the focus; a click on the focused cover opens it.
   - Under it: title, artist, a quiet line ("Playing now", "Played 2 hours ago · stopped in track 4 of 9") and two buttons: **Resume** (continues that context from the track where it stopped; for the context that is playing: **Now Playing**) and **Open album** (or playlist/mix). Dots under it show the position.
   - **The page's glow** follows the focused cover (see Colour), crossfading over ~1.8s.
2. **Moods for today**: three cards side by side (one column when narrow). Each is a quick mix of three albums in one genre that the listener plays a lot, chosen from albums not heard in a while:
   - The card: the three covers fanned (they spread a little on hover), a small label ("46 albums in your library"), the genre name large, the three album titles, a line of facts ("2 h 14 min · not played since August"; "one you've never played"), and a round **Play** button. Tinted with its albums' colours. A click on a cover opens that album.
   - **Picking** (detail in the slice brief): the genres come from the play history (the most played genres over recent weeks, at most one card per genre, no two cards the same genre); per genre three albums, preferring ones not played for longest and never ones played this week. With too little history, the source's largest genres stand in. The picks stay the same all day (new at local midnight, cached per source and day like 1.x's daily picks); **Other moods** shows the next three genres.
   - **Play** starts a quick mix with the three albums as ingredients (the mix machinery, ADR 0008), so it plays on as long as the listener lets it.
- Not on Home: Recently added, Most played and Daily Picks rows (1.x). Recently added and Most played live in Library as sorts; most played songs on artist pages.
- A new listener (no history): the carousel is replaced by a short welcome line and **Shuffle your library** / **Browse albums**; Moods use the largest genres.

## Library
- Header: "Library" (display type), the section control, and per section its tools: a **sort** (Albums: A–Z, Most played, Recently added, Year; Navidrome offers the server's sorts) and a **Covers / List** toggle (Albums, Artists, a genre's albums), remembered per section. The Preferences default applies until a section has its own choice.
- **Covers**: a responsive grid (tiles ≥178px), the cover with a soft shadow, title and "artist · year" below. Hover lifts the cover slightly and shows a round Play button in the album's own accent. Artists as round images.
- **List**: dense rows (52px: small cover, title, artist, genre, year or plays when sorted by Most played, length). For collections of thousands; virtualized as today.
- **Genres**: tiles tinted with the genre's best-known cover's colours, the name in display type, album count. **Playlists**: cards with a 2×2 cover mosaic. **Mixes**: cards with ingredient chips and Play, the playing mix marked; New mix… (the 1.x editor, restyled).
- **Album page**: a header with the cover (~270px), "Album · year" label, the title in large display type, the artist (a link), "N songs · length · genres" (genres as links), **Play** (accent), **Shuffle**, "…" (the album menu: Add to mix…, Add to playlist…). The page glows with the album's colours. Then the track list: number, title, length; the playing row in the accent with the equaliser glyph.
- **Artist page**: round image, name, counts, Play / Shuffle; **Your most played** (up to six of the artist's songs by your play count, with the count; hidden without plays); then the albums.
- **Genre, playlist and mix pages**: the same header pattern (the genre and playlist glow with their first cover's colours).

## Search
- One large rounded field at the top ("Artists, albums, songs or lyrics"), focused on arrival (`/` or `Ctrl+F` from anywhere). Empty: the source's genres as chips to jump to.
- Results: **Top result** (a card) beside **Songs**, then Albums (grid) and Artists, then **Lyrics** (1.x rules, below). Searches the active source, with "Search … instead".
- **Lyrics** (carried over from 1.x): local, and Navidrome while its lyrics index is on. Track rows with a third, dimmer line holding the lyric line or the two lines the match spans (joined by " / "), the matched phrase at weight 600 (no highlight colour). Shown once loaded (no spinner), never for queries under 3 characters, "and N more" past 50 matches. On Navidrome, while songs are left to index, a quiet note follows the results: "Lyrics are still being indexed (1,200 of 2,400 songs)"; while the index loads: "The lyrics index is still loading."

## Player
- The strip, left to right: the cover (54px, rounded; a click opens Now Playing windowed), title and "artist · album" (names are links as in 1.x); centred: previous / play-pause (a filled circle) / next over the timeline (elapsed, seek bar, total, **repeat** after the total time as in 1.x); right: a small visualizer (5 bars, the playing accent), volume, **Queue**, **Now Playing** (full screen).
- **Its background is the playing cover**, enlarged and heavily blurred under a tint of the playing colours, over the frosted glass; it crossfades (~1.6s) on a track change. Text stays at body contrast over it (the tint guarantees it; see Colour).
- Narrow windows drop the volume and visualizer, then the timeline and previous/next.

## Queue
- **It rises out of the player**: `q` or the player's Queue button opens a panel anchored to the player's right edge, directly above it (≤400px wide, up to ~560px tall), springing up from the button with a short scale-and-rise; Esc, `q`, the button or a click elsewhere closes it. Tinted glass with the playing colours.
- Contents as 1.x: "Now playing", then "Next from <context>"; selection, drag-and-drop reordering, the header buttons (Shuffle upcoming, Save queue as playlist with Navidrome, Clear queue, asks first), the mix line ("Mix: <name>", loading state).
- In **Now Playing** the queue stays the side panel of 1.x (Now Playing has no player strip to attach it to).

## Now Playing
Carried over from 1.x, which this design grew from, with three changes: it opens with the **player's cover morphing into its cover** (and back on close); its background tint and accent are the playing colours (no separate always-dark `--np-*` tokens: the base is dark by design); and the visualizer uses the playing accent.

- Immersive: only the view remains. Large cover, title/artist/album (the title always takes two lines' height, its text at the bottom, so nothing below moves between tracks), one timeline (repeat beside it), one transport row (previous / play-pause / next, volume on the right), visualizer bars and a small **Up next** card (cover, title, artist; clicking it opens the queue; while the queue is open it fades out but keeps its place). Background: a heavily blurred, darkened copy of the cover. Top right: lyrics, queue, Full screen / Exit full screen, Close (Esc or `f`). `f` and the player's Full screen button open it in full screen; a click on the player's cover opens it windowed. Closing leaves full screen only if opening it (or its toggle) entered it. The queue is a translucent ~360px side panel sliding in from the right (`q`), its open/closed state remembered apart from the main queue's; wide windows make room for it, narrow ones let it overlay. All global shortcuts keep working.
- **Lyrics mode** (`l`, or the lyrics button; tooltip "Lyrics (l)"): on for the session once turned on, off at launch. The player stacks into a column on the left (~300–340px); the lyrics fill the middle, at most ~40ch wide, centred, fading out at the top and bottom edges; the queue panel slides in over the right of the lyrics area, and the text moves left (and in narrow windows wraps narrower, down to 18ch) only as far as needed to clear it. Timed lyrics follow the song: the current line in the text colour, the others dimmed (colour only), kept about a third of the way down with a smooth scroll; any scroll the view did not make pauses that for ~4s, then it eases back, never while the scrollbar is held; clicking a line seeks there; lines are not Tab stops. Untimed lyrics are plain text. No lyrics: a dim "No lyrics for this track"; a failure: "Couldn't load lyrics"; loading shows nothing for 300ms, then "Loading lyrics…". While paused the highlight stays.
- **Mixes**: "Mix: <name>" (or "Quick mix: <name>", which includes Moods) above the title, with a loading state.
- Motion: as 1.x (Motion below).

## Preferences
One page of grouped glass cards, the title "Make it yours":
- **Accent**: "From the music" (default; a page takes the colours of the album it shows, the player those of the song that plays) or one of ~6 curated colours, or a custom colour (hex + picker). A fixed colour is used everywhere.
- **Music source**: Navidrome / Local folder (when both are set up), with the status line (connection, or the scan's "N tracks" / "Scanning… N"); the server's and the folder's settings (1.x Settings, restyled).
- **Library**: show albums and artists as Covers or List by default.
- **Lyrics search** (with Navidrome): as 1.x (the switch, the live status row, Rebuild index).
- Onboarding (first run) keeps its 1.x flow, restyled to this design.

## Colour
- **Base**: a dim, slightly violet-tinted near-black (`--base`, ~`hsl(256 14% 7%)`), never a neutral grey. Text: a warm white (`--ink`) with two quieter levels (`--ink-2` ~68%, `--ink-3` ~44%). Surfaces are **glass**: translucent white (5–11%) with a hairline border (9%) and a backdrop blur where something floats over content. There is no light or dark mode.
- **Music colours**: for every cover the backend computes a small palette (`tint`: a deep dark of its dominant hue for backgrounds; `accent`: its most vivid colour, kept within a lightness and chroma band so it reads on the base and on its own tint; `accentInk`: black or white by contrast; `glow`: the accent, translucent). They are computed once per cover and cached with it.
- **Who takes which colours** (with "From the music"):
  - The **player, the queue and Now Playing**: the playing cover's.
  - A **page** (`#view`): what it shows: Home the carousel's focused album; an album, artist, genre or playlist page its (first) cover's. The page's **glow** is that cover, very blurred, behind the top of the page, fading out over ~600px so the rest of the page (Moods, track lists) rests on the base. It scrolls with the page.
  - **Small things that stand for one album** (a grid tile's Play button, a mood card) use that album's accent.
  - Pages without one subject (Search, Library lists, Preferences): the base and the **house accent** (Coda amber, ~`#f0a83a`).
- **Blending**: colour changes are transitions of registered custom properties (`@property` for `--accent`, `--glow`, `--tint`): ~1.6s for accents and glows, ~2s for tints; covers crossfade. Where the webview cannot animate custom properties, colours switch at once while the covers still crossfade.
- **Contrast**: body text ≥4.5:1 on the base, on glass and on every tint; secondary text ≥4.5:1; faint text, accent and danger ≥3:1. Tints are dark enough by construction; computed accents are nudged in OKLCH lightness until they pass on the base and on their tint. A custom accent the listener picks is used as is, with a small contrast warning in Preferences.
- **No colour literals outside the token layer** (as 1.x): components use `var(--…)`; the values live in `src/styles/tokens.css`; `pnpm check:colors` enforces it. Danger and success keep their own colours, separate from the accent.

## Type
- **Provisional; a separate exploration picks the final pair.** The mockup's pairing: **Fraunces** (soft, slightly quirky serif, `SOFT 100, WONK 1`, ~620–640 weight) for display text: page titles, album and genre names, section headings, the wordmark; **Figtree** (friendly rounded sans) for everything else. Both OFL; **bundled with the app** (woff2 in the frontend), never loaded from the web.
- Small labels (the "eyebrows" above titles: "Album · 1977", "Thursday evening") are the display face in italic at ~15px, sentence case; no tracked-out uppercase labels.
- Tabular numerals for times, counts and track numbers. Scale: 14.5px body, 13px secondary; section headings ~30px; page titles 36–72px by width; the carousel title ~32px.

## Shape and surfaces
- Radii: covers 14–18px (12px in the player, round for artists), cards and panels 22–30px, buttons and the navigation fully round (pills).
- Shadows are soft and long; a focused or featured cover also casts a glow in its accent.
- Buttons: **primary** a pill in the accent with `accentInk` text; **secondary** a glass pill; icon buttons round, glass where they float.
- Covers without art: the 1.x generated placeholder (initials on a gradient), its colours taken from the house palette.
- Icons: inline SVG (`src/components/icons.tsx`), 18–20px, `currentColor`, rounded caps and joins.
- **The flourish (open)**: the mockup slides a record out of the carousel's focused sleeve (its label the cover; it spins while that album plays). Decided in the refinement round; if it stays, it is the only flourish of its kind.

## Motion
- The 1.x rules hold: only `transform` and `opacity` animate (colours through registered properties); nothing holding focus or state is remounted for an animation; costly blurred layers are not repainted by it; View Transitions where the webview has them, a short crossfade where it does not; with `prefers-reduced-motion: reduce` nothing animates (colours switch, covers swap).
- Curves: `--ease` (cubic-bezier(.22,.7,.18,1)) for glides; `--spring` (cubic-bezier(.34,1.56,.64,1), a slight overshoot) only for small direct responses (a button press, the queue rising, a cover lifting on hover).
- **New in 2.0**:
  - **Navigation indicator** glides between Home, Search and Library (~0.5s).
  - **Cover morphs** (View Transitions): a grid or carousel cover into the album page's header cover and back; the player's cover into Now Playing's cover and back. Other page changes crossfade (~0.35s).
  - **Carousel**: covers glide and rescale (~0.9s); the meta line below fades up on change.
  - **Glows and tints** crossfade slowly (Colour).
  - **Queue**: rises and scales in from the player (~0.5s, `--spring`), fades out faster.
- **Now Playing, carried over from 1.x**: opening fades it in and scales it from 0.98 (with the cover morph from the player); the queue panel slides in and out from the right, and where the layout changes with it the cover morphs and the text crossfades; Up next fades out (with a small drop) while the queue is open; **lyrics mode** switches in about 1s (everything but the cover fades out where it is, ~160ms; the cover glides to its new place, ~380ms; the new layout's parts fade in rising ~10px, staggered ~70ms; the lyrics rise in from ~40px below, ~500ms); **track change** slides only the cover and the title lines about 64px with a fade (left for a later track, right for an earlier one; restarting the same entry does not slide), waiting up to ~150ms for the new cover to decode; the backdrop crossfades. Timings in `experience.css` (`--np-lx-*`, `--motion-*`).

## Interaction
- Double-click or Enter on a track plays its context from that track. Context menu: Play, Play next, Add to queue, Go to album, Go to artist, Add to playlist…, Add to mix… (where 1.x has it).
- Artist and album names open their page on click wherever an id is known: plain text that turns the accent colour on hover, no underline, the default cursor. In rows and cards a click on a name does not select, play or drag the row, and adds no Tab stop; in the player and Now Playing they are Tab stops. (1.x rules, unchanged.)
- **Mix editor** and **Add to mix…** / **Add to playlist…** pickers: the 1.x dialogs and their keyboard rules, restyled as glass dialogs.
- Keyboard (global, unless typing in an input): `Space` play/pause, `n`/`p` or `Ctrl+→/←` next/previous, `←/→` seek ∓5s (on Home, ←/→ move the carousel while it has focus), `+`/`-` volume, `r` repeat, `s` shuffle the context, `q` the queue, `f` Now Playing (full screen), `l` lyrics mode (in Now Playing), `/` or `Ctrl+F` search, `Ctrl/Cmd+1`/`2` Local/Navidrome, `Esc` closes the topmost thing (queue, Now Playing, a dialog), else Back.
- Notices: small glass toasts bottom-right, above the player; persistent problems (mpv missing, Navidrome unreachable) as a banner at the top of the page.
