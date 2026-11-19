# Coda App — Design (2.0, "Neo-Coda")

**Status: the 2.0 design, built in slices N1–N7 (`PLAN.md`, "2.0"); every section is shipped.** Still open, for the refinement rounds with the owner: the final typefaces (Type), the one flourish (the record, or none: Shape and surfaces, Home), the owner's nitpick round, and the 2.0.0 release. The 1.x design is kept at the tag `coda-gui-v1.0.0` (`git show coda-gui-v1.0.0:apps/coda-gui/docs/DESIGN.md`). Each section says *(shipped)* with the slice that built it. The direction and the retired theme system: ADR 0009. The reference mockup (invented music, generated covers): https://claude.ai/artifact/VpXdkrF5jpKBoNJ8va2Ez3 (owner-only).

## Principles
- **A native app, not a web page.** No link styling, no hover underlines, text not selectable except in inputs, the default cursor on controls, right-click context menus where they help. Views scroll on their own; the chrome floats over them.
- **Designed, not configurable.** One opinionated look. Personalisation is the accent colour (the music's, or one fixed colour), not themes.
- **Colour belongs to what it is attached to.** A page takes the colours of what it shows, the player those of what plays; everything else rests on a calm, dim base. Colours blend slowly; nothing flashes.
- **Roomy by default, dense on request.** Big covers and air; every long collection has a list view.
- **Calm motion with one moment at a time.** Things glide and settle; nothing bounces for attention. At most one decorative flourish in the whole app.
- **The music is the decoration.** Covers, their colours and the visualizer carry the look; the chrome stays quiet glass.

## Layout
*(Shipped. N1: the top bar, its navigation and Preferences button, the macOS window buttons in its row, no sidebar and no queue column; N2: the glass; N3: the page's glow; N4: the floating player, the page's body scrolling under it, the queue rising out of it; N5a: the page scrolling under the top bar as one scroll container (its header scrolls away, a list's tools stick under the bar as glass, its glow scrolls with it), content ≤1240px centred.)*

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

## Navigation *(shipped)*
- Three roots: **Home**, **Search**, **Library**. Each keeps its own back stack (Back: `Backspace`, `Alt+←`, the mouse's back button, Esc when nothing is open, or the page's ‹ Back): Library → Artist → Album, Home → Album, …
- **Library** holds the collection as sections in a segmented control at the top: **Albums, Artists, Genres, Playlists, Mixes**, and **Folders** for a local library. The chosen section is remembered.
- **Preferences** (the slider icon, top right) is a page of its own, outside the three roots.
- **One source at a time** stays, but leaves the main UI: the source is chosen in Preferences (and with `Ctrl/Cmd+1`/`2`). With one source configured there is nothing to choose. The choice is saved in `~/.config/coda-app/settings.toml` as today (default: Navidrome if configured, else Local; a source that goes away falls back to the other). Search's "Search … instead" link and "Go to album/artist" on a track of the other source (switches first) stay. Back stacks are per source, as in 1.x.
- **Now Playing** covers everything (`f`, the player's expand button, or a click on the player's cover).

## Home *(shipped, N6; the record flourish open)*
The landing page. Two things, and room around them.

1. **Where you left off** (heading, with the day and part of the day above it in the small italic label style, e.g. "Thursday evening"). A carousel of the ~8 most recent listening contexts (albums first; playlists and mixes as their cover or mosaic), newest first:
   - The focused cover is large (~30% of the page width, 180–330px), centred; its neighbours smaller (×0.72, then ×0.6) and dimmed, fading out by the fourth. ←/→ (only while the carousel has the focus: a cover, which a click on a cover or on ‹ › gives it; anywhere else on Home they seek as on every page), the ‹ › buttons, a horizontal trackpad swipe, or a click on a neighbour moves the focus; a click on the focused cover opens it.
   - Under it: title, artist, a quiet line ("Playing now", "Played 2 hours ago · stopped in track 4 of 9") and two buttons: **Resume** (continues that context from the track where it stopped; for the context that is playing: **Now Playing**) and **Open album** (or playlist/mix). Dots under it show the position.
   - **The page's glow** follows the focused cover (see Colour), crossfading over ~1.8s.
2. **Moods for today**: three cards side by side (one column when narrow). Each is a quick mix of three albums in one genre that the listener plays a lot, chosen from albums not heard in a while:
   - The card: the three covers fanned (they spread a little on hover), a small label ("46 albums in your library"), the genre name large, the three album titles, a line of facts ("2 h 14 min · not played since August"; "one you've never played"), and a round **Play** button. Tinted with its albums' colours. A click on a cover opens that album.
   - **Picking** (detail in the slice brief, `docs/plans/n6-home.md`; the rules in `src-tauri/src/history/moods.rs`): the genres come from the play history (the most played genres over recent weeks, at most one card per genre, no two cards the same genre); per genre three albums, preferring ones not played for longest and never ones played this week. With too little history, the source's largest genres stand in. The picks stay the same all day (new at local midnight, cached per source and day like 1.x's daily picks); **Other moods** shows the next three genres.
   - **Play** starts a quick mix with the three albums as ingredients (the mix machinery, ADR 0008), so it plays on as long as the listener lets it; the mix line reads "Quick mix: <Genre> mood".
- Not on Home: Recently added, Most played and Daily Picks rows (1.x). Recently added and Most played live in Library as sorts; most played songs on artist pages.
- A new listener (no history): the carousel is replaced by a short welcome line and **Shuffle your library** / **Browse albums**; Moods use the largest genres.

## Library
*(Shipped. N5a: the Library header ("Library", the section control and the section's tools in a tool row that sticks under the top bar) and the page header pattern of the album, artist, genre and playlist pages (and a folder's, without a cover), "…" holding the page's menu. N5b: Covers / List for Albums, Artists and a genre's albums (remembered per section and source, the Preferences default until then), the sorts below (local: A–Z, Artist, Year, Most played by this app's listens), the per-tile accents and the cover glow, the List's sortable columns, Your most played on artist pages. N7: Genres as tinted tiles and Playlists as mosaic cards, each with Covers / List; the Mixes cards restyled.)*

- Header: "Library" (display type), the section control, and per section its tools: a **sort** (Albums: A–Z, Most played, Recently added, Year; Navidrome offers the server's sorts) and a **Covers / List** toggle (Albums, Artists, a genre's albums), remembered per section. The Preferences default applies until a section has its own choice.
- **Covers**: a responsive grid (tiles ≥178px), the cover with a soft shadow, title and "artist · year" below. Hover lifts the cover slightly and shows a round Play button in the album's own accent. Artists as round images.
- **List**: dense rows (52px: small cover, title, artist, genre, year or plays when sorted by Most played, length). For collections of thousands; virtualized as today.
- **Genres** *(shipped, N7)*: tiles in a responsive grid (≥200px), each tinted with its first album's cover's colours (that cover's tint as a wash; the house glass until known; a Navidrome genre's cover is looked up while its tile is near the view, one genre at a time), the album count, the name in display type, that cover peeking tilted from the top-right corner. Covers / List (List: the 1.x rows with counts). **Playlists** *(shipped, N7)*: cards with a 2×2 mosaic of the covers of its first four different albums (local; Navidrome's own playlist cover is the server's mosaic), name, song count; the page header shows the same mosaic; Covers / List. **Mixes**: cards with ingredient chips and Play, lifting a little on hover, the playing mix outlined in the accent with its glow and the equaliser; New mix… (the 1.x editor, restyled).
- **Album page**: a header with the cover (~270px), "Album · year" label, the title in large display type, the artist (a link), "N songs · length · genres" (genres as links), **Play** (accent), **Shuffle**, "…" (the album menu: Add to mix…, Add to playlist…). The page glows with the album's colours. Then the track list: number, title, length; the playing row in the accent with the equaliser glyph.
- **Artist page**: round image, name, counts, Play / Shuffle; **Your most played** (up to six of the artist's songs by your play count, with the count; hidden without plays); then the albums.
- **Genre, playlist and mix pages**: the same header pattern (the genre and playlist glow with their first cover's colours).

## Search *(shipped, N5a)*
- One large rounded field at the top ("Artists, albums, songs or lyrics"), focused on arrival (`/` or `Ctrl+F` from anywhere). Empty: the source's genres as chips to jump to.
- Results: **Top result** (a card: the artist or album named most like the query) beside **Songs** (the first five; "Show all" lists every match), then Albums (grid) and Artists (round images), then **Lyrics** (1.x rules, below). Searches the active source, with "Search … instead".
- **Lyrics** (carried over from 1.x): local, and Navidrome while its lyrics index is on. Track rows with a third, dimmer line holding the lyric line or the two lines the match spans (joined by " / "), the matched phrase at weight 600 (no highlight colour). Shown once loaded (no spinner), never for queries under 3 characters, "and N more" past 50 matches. On Navidrome, while songs are left to index, a quiet note follows the results: "Lyrics are still being indexed (1,200 of 2,400 songs)"; while the index loads: "The lyrics index is still loading."

## Player *(shipped, N4)*
- The strip, left to right: the cover (54px, rounded; a click opens Now Playing windowed), title and "artist · album" (names are links as in 1.x); centred: previous / play-pause (a filled circle) / next over the timeline (elapsed, seek bar, total, **repeat** after the total time as in 1.x); right: a small visualizer (5 bars, the playing accent), volume, **Queue**, **Now Playing** (full screen).
- **Its background is the playing cover**, enlarged and heavily blurred under a tint of the playing colours, over the frosted glass; it crossfades (~1.8s, the glows' crossfade; its colours blend in ~1.6s, the tint in ~2s) on a track change. Text stays at body contrast over it (the tint guarantees it; see Colour): over an all-white cover text ≥10.9:1, secondary ≥5.2:1, faint ≥3.3:1.
- Nothing playing: the strip stays, with a quiet "Nothing playing" and the controls disabled as far as there is nothing to control.
- Narrow windows (below ~860px) drop the volume and visualizer, then (below ~600px, narrower than the minimum window) the timeline and previous/next. The visualizer's readings flow only while it is drawn.

## Queue *(shipped, N4)*
- **It rises out of the player**: `q` or the player's Queue button opens a panel anchored to the player's right edge, directly above it (≤400px wide, up to ~560px tall), springing up from the button with a short scale-and-rise; Esc, `q`, the button or a click elsewhere closes it. Tinted glass with the playing colours.
- Contents as 1.x: "Now playing", then "Next from <context>"; selection, drag-and-drop reordering, the header buttons (Shuffle upcoming, Save queue as playlist with Navidrome, Clear queue, asks first), the mix line ("Mix: <name>", loading state).
- In **Now Playing** the queue stays the side panel of 1.x (Now Playing has no player strip to attach it to).

## Now Playing *(shipped: 1.x, N3 colours, N4 morph)*
Carried over from 1.x, which this design grew from, with three changes: it opens with the **player's cover morphing into its cover** (and back on close; *shipped, N4*: from the player's cover, windowed; `f` and the Full screen button open it without the morph, as the window resizes into full screen meanwhile, and a close that leaves full screen does not morph either); its background tint and accent are the playing colours (no separate always-dark `--np-*` tokens: the base is dark by design); and the visualizer uses the playing accent.

- Immersive: only the view remains. Large cover, title/artist/album (the title always takes two lines' height, its text at the bottom, so nothing below moves between tracks), one timeline (repeat beside it), one transport row (previous / play-pause / next, volume on the right), visualizer bars and a small **Up next** card (cover, title, artist; clicking it opens the queue; while the queue is open it fades out but keeps its place). Background: a heavily blurred, darkened copy of the cover. Top right: lyrics, queue, Full screen / Exit full screen, Close (Esc or `f`). `f` and the player's Full screen button open it in full screen; a click on the player's cover opens it windowed. Closing leaves full screen only if opening it (or its toggle) entered it. The queue is a translucent ~360px side panel sliding in from the right (`q`), its open/closed state remembered apart from the main queue's; wide windows make room for it, narrow ones let it overlay. All global shortcuts keep working.
- **Lyrics mode** (`l`, or the lyrics button; tooltip "Lyrics (l)"): on for the session once turned on, off at launch. The player stacks into a column on the left (~300–340px); the lyrics fill the middle, at most ~40ch wide, centred, fading out at the top and bottom edges; the queue panel slides in over the right of the lyrics area, and the text moves left (and in narrow windows wraps narrower, down to 18ch) only as far as needed to clear it. Timed lyrics follow the song: the current line in the text colour, the others dimmed (colour only), kept about a third of the way down with a smooth scroll; any scroll the view did not make pauses that for ~4s, then it eases back, never while the scrollbar is held; clicking a line seeks there; lines are not Tab stops. Untimed lyrics are plain text. No lyrics: a dim "No lyrics for this track"; a failure: "Couldn't load lyrics"; loading shows nothing for 300ms, then "Loading lyrics…". While paused the highlight stays.
- **Mixes**: "Mix: <name>" (or "Quick mix: <name>", which includes Moods) above the title, with a loading state.
- Motion: as 1.x (Motion below).

## Preferences
*(Shipped. N2: the page, its title and glass cards; Accent with the house amber, five more curated colours and a custom colour (hex field and the native picker, a contrast note under 3:1), saved in `settings.toml` and applied before the first paint; Music source; the server, folder, cover art/scrobbling and lyrics search sections restyled. N3: "From the music", first and the default. N5b: the Library card (Covers or List by default). N7: onboarding restyled.)*

One page of grouped glass cards, the title "Make it yours":
- **Accent**: "From the music" (default; a page takes the colours of the album it shows, the player those of the song that plays) or one of ~6 curated colours, or a custom colour (hex + picker). A fixed colour is used everywhere.
- **Music source**: Navidrome / Local folder (when both are set up), with the status line (connection, or the scan's "N tracks" / "Scanning… N"); the server's and the folder's settings (1.x Settings, restyled).
- **Library**: show albums and artists as Covers or List by default.
- **Lyrics search** (with Navidrome): as 1.x (the switch, the live status row, Rebuild index).
- Onboarding (first run) keeps its 1.x flow, restyled to this design *(N7)*: a glass card over the base with a soft glow in the accent behind it (28% at its centre; text on the card ≥4.5:1, faint text ≥4:1 even with a white custom accent), the coda sign and wordmark, display type for its headings, steps separated by hairlines, pill buttons.

## Colour
*(Shipped: N2 the base, the text levels, glass and the house accent, in `tokens.css`; the native window background is the base. N3 the music colours, who takes which (the player, the queue, Now Playing and pages; small things (a grid tile's Play button and glow) in N5b; Home's carousel and the mood cards in N6), blending and the page glow. "From the music" is the default accent.)*

- **Base**: a dim, slightly violet-tinted near-black (`--base`, ~`hsl(256 14% 7%)`), never a neutral grey. Text: a warm white (`--ink`) with two quieter levels (`--ink-2` ~68%, `--ink-3` ~44%). Surfaces are **glass**: translucent white (5–11%) with a hairline border (9%) and a backdrop blur where something floats over content. There is no light or dark mode.
- **Music colours** *(shipped, N3)*: for every cover the backend computes a small palette (`tint`: a deep dark of its dominant hue for backgrounds; `accent`: its most vivid colour, kept within a lightness and chroma band so it reads on the base and on its own tint; `accentInk` (`accentText` in the code): black or white by contrast; `glow`: the accent, translucent; a greyscale cover is not `vivid` and keeps the house accent). They are computed once per cover and cached with it (in memory, by cover).
- **Who takes which colours** (with "From the music"; *shipped in N3* for the player, the queue, Now Playing and the album, artist, genre and playlist pages; the glow scrolling with the page in N5a; small things *shipped in N5b*; Home's focused cover and the mood cards *shipped in N6*):
  - The **player, the queue and Now Playing**: the playing cover's.
  - A **page** (`#view`): what it shows: Home the carousel's focused album; an album, artist, genre or playlist page its (first) cover's. The page's **glow** is that cover, very blurred, behind the top of the page, fading out over ~600px so the rest of the page (Moods, track lists) rests on the base. It scrolls with the page.
  - **Small things that stand for one album** (a grid tile's Play button, a mood card) use that album's accent.
  - Pages without one subject (Search, Library lists, Preferences): the base and the **house accent** (Coda amber, ~`#f0a83a`).
- **Blending** *(shipped, N3)*: colour changes are transitions of registered custom properties (`@property` for `--accent`, `--glow`, `--tint`): ~1.6s for accents and glows, ~2s for tints; covers crossfade. Where the webview cannot animate custom properties, colours switch at once while the covers still crossfade.
- **Contrast**: body text ≥4.5:1 on the base, on glass and on every tint; secondary text ≥4.5:1; faint text, accent and danger ≥3:1. Tints are dark enough by construction; computed accents are nudged in OKLCH lightness until they pass on the base and on their tint. A custom accent the listener picks is used as is, with a small contrast warning in Preferences.
- **No colour literals outside the token layer** (as 1.x): components use `var(--…)`; the values live in `src/styles/tokens.css`; `pnpm check:colors` enforces it. Danger and success keep their own colours, separate from the accent.

## Type *(shipped, N2; the final pair open)*
- **Provisional; the owner's type exploration (open, after N7) may pick another pair.** The mockup's pairing, bundled in N2: **Fraunces** (soft, slightly quirky serif, `SOFT 100, WONK 1`, ~620–640 weight) for display text: page titles, album and genre names, section headings, the wordmark; **Figtree** (friendly rounded sans) for everything else. Both OFL; **bundled with the app** (the variable TrueType files from the google/fonts repository in `public/fonts/`, with their `OFL.txt`), never loaded from the web.
- Small labels (the "eyebrows" above titles: "Album · 1977", "Thursday evening") are the display face in italic at ~15px, sentence case; no tracked-out uppercase labels.
- Tabular numerals for times, counts and track numbers. Scale: 14.5px body, 13px secondary; section headings ~30px; page titles 36–72px by width; the carousel title ~32px.

## Shape and surfaces *(shipped, N2; the accent glow on grid covers N5b, on hover or focus; the flourish open)*
- Radii: covers 14–18px (12px in the player, round for artists), cards and panels 22–30px, buttons and the navigation fully round (pills).
- Shadows are soft and long; a focused or featured cover also casts a glow in its accent.
- Buttons: **primary** a pill in the accent with `accentInk` text; **secondary** a glass pill; icon buttons round, glass where they float.
- Covers without art: the 1.x generated placeholder (initials on a gradient), its colours taken from the house palette.
- Icons: inline SVG (`src/components/icons.tsx`), 18–20px, `currentColor`, rounded caps and joins.
- **The flourish (open)**: the mockup slides a record out of the carousel's focused sleeve (its label the cover; it spins while that album plays). Decided in the refinement round; if it stays, it is the only flourish of its kind.

## Motion *(shipped, N4–N7)*
- The 1.x rules hold: only `transform` and `opacity` animate (colours through registered properties); nothing holding focus or state is remounted for an animation; costly blurred layers are not repainted by it; View Transitions where the webview has them, a short crossfade where it does not; with `prefers-reduced-motion: reduce` nothing animates (colours switch, covers swap).
- Curves: `--ease` (cubic-bezier(.22,.7,.18,1)) for glides (page changes, cover morphs, the navigation indicator, the carousel, the mood covers fanning); `--spring` (cubic-bezier(.34,1.56,.64,1), a slight overshoot) only for small direct responses (a button press, the queue rising, a cover lifting on hover, a tile's Play button scaling in). Durations are tokens (`tokens.css` `--motion-*`), all 0 with reduced motion.
- **New in 2.0**:
  - **Navigation indicator** glides between Home, Search and Library (~0.5s, `--motion-nav`, `--ease`) *(shipped, N1; N7 curve)*.
  - **Page changes** *(shipped, N7)*: every change of the shown page (push, Back, a root, a Library section, a sort) is a View Transition: the new page fades in over the old (~0.35s, `--motion-page`); the top bar, the player and an open queue panel have names of their own and do not move or fade. When the new page has a scroll position to restore (Back), or the cover to morph into is not there yet, the new screen waits for the page to settle (loaded, empty or failed) and then for the cover to decode, all within one ~150ms deadline (the screen holds the old one meanwhile), so nothing jumps after it; anything else starts at once. A newer navigation skips the running transition to its end (holding Backspace never queues fades). Without View Transitions, or with reduced motion, pages switch at once.
  - **Cover morphs** (View Transitions): opening an album, artist or playlist from its cover (a grid tile, a list row's cover, Home's focused carousel cover (also through Open), a mood card's cover, Search's Top result and artist tiles, a playlist card's cover or mosaic; also a placeholder without art) morphs that cover into the page header's cover (~0.38s, `--ease`) while the page crossfades; Back morphs it back into the same cover when it is on screen at the restored place, else a plain crossfade *(shipped, N7)*. One element at a time carries the morph's name, set just for the transition and cleared after (also when a newer one interrupts). The player's cover into Now Playing's cover and back *(shipped, N4: ~0.38s; the rest crossfades, ~0.3s; without View Transitions Now Playing fades and scales in by itself)*.
  - **Carousel** *(shipped, N6)*: covers glide and rescale (~0.9s, `--motion-carousel`, `--ease`); the meta line below fades up on change.
  - **Glows and tints** crossfade slowly (Colour).
  - **Queue**: rises and scales in from the player's Queue button (~0.5s, `--spring`), fades out faster (~0.25s) *(shipped, N4)*.
- **Now Playing, carried over from 1.x**: opening from the player's cover morphs the cover into place while the rest crossfades; where the morph does not run (`f` and the Full screen button, without View Transitions) it fades in and scales from 0.98 instead; the queue panel slides in and out from the right, and where the layout changes with it the cover morphs and the text crossfades; Up next fades out (with a small drop) while the queue is open; **lyrics mode** switches in about 1s (everything but the cover fades out where it is, ~160ms; the cover glides to its new place, ~380ms; the new layout's parts fade in rising ~10px, staggered ~70ms; the lyrics rise in from ~40px below, ~500ms); **track change** slides only the cover and the title lines about 64px with a fade (left for a later track, right for an earlier one; restarting the same entry does not slide), waiting up to ~150ms for the new cover to decode; the backdrop crossfades. Timings in `experience.css` (`--np-lx-*`, `--motion-*`).

## Interaction *(shipped)*
- Double-click or Enter on a track plays its context from that track. Context menu: Play, Play next, Add to queue, Go to album, Go to artist, Add to playlist…, Add to mix… (where 1.x has it).
- Artist and album names open their page on click wherever an id is known: plain text that turns the accent colour on hover, no underline, the default cursor. In rows and cards a click on a name does not select, play or drag the row, and adds no Tab stop; in the player and Now Playing they are Tab stops. (1.x rules, unchanged.)
- **Mix editor** and **Add to mix…** / **Add to playlist…** pickers: the 1.x dialogs and their keyboard rules, restyled as glass dialogs.
- Keyboard (global, unless typing in an input): `Space` play/pause, `n`/`p` or `Ctrl+→/←` next/previous, `←/→` seek ∓5s (on Home, ←/→ move the carousel while it has focus), `+`/`-` volume, `r` repeat, `s` shuffle the context, `q` the queue, `f` Now Playing (full screen), `l` lyrics mode (in Now Playing), `/` or `Ctrl+F` search, `Ctrl/Cmd+1`/`2` Local/Navidrome, `Esc` closes the topmost thing (queue, Now Playing, a dialog), else Back.
- Notices: small glass toasts bottom-right, above the player; persistent problems (mpv missing, Navidrome unreachable) as a banner at the top of the page.
