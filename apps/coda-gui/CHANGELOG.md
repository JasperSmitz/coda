# Coda App changelog

Notable changes to the desktop app (`apps/coda-gui`). The terminal player has its own changelog at the repository root. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the app uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) and is tagged `coda-gui-v*`.

## [2.0.0] - 2026-11-20 (Neo-Coda)

A redesign of the app around the music, in the spirit of Now Playing: one designed look, coloured by your covers. The app keeps the 1.x engine, sources, queue and keyboard paths. The design is in `docs/DESIGN.md`, and the decision behind it is in ADR 0009.

### Changed
- **Navigation:**
  - A floating top bar with **Search · Home · Library**. Home sits in the middle, and a two-finger sideways swipe moves one step left or right.
  - Albums, Artists, Genres, Playlists, Mixes and Folders are sections inside Library.
  - The Local/Navidrome switch moved to Preferences.
- **Home:**
  - **Where you left off**: a carousel of what you played recently, with Resume (it continues from the track and position where you stopped).
  - **Moods for today**: three daily quick mixes from the genres you listen to most, three albums each.
  - Recently added, Most played and Daily Picks are gone.
- **Colour from the music:**
  - A page takes the colours of what it shows: the ground blends up to three of the cover's colours, and the cover's art blooms out behind it.
  - The player, the queue and Now Playing follow the playing song.
  - Colours blend as they change. Each is chosen per cover so text stays readable.
- **Player:** a floating player with the playing cover blurred behind it. The queue rises out of it, and its cover morphs into Now Playing.
- **Library:**
  - Covers or List for albums and artists, with new sorts (Most played, Recently played and more).
  - Your most played songs on artist pages.
  - Large page headers with Play, Shuffle and the "…" menu.
  - Pages scroll under the top bar and crossfade, and covers morph into page headers.
- **Search:** a large search field, with a top result and songs, then albums, artists and lyrics.
- **Preferences:**
  - Accent (from the music, or a fixed colour).
  - Covers or List as the default.
  - **Start on** (Search, Home or Library).
- **Type:** Fraunces and Figtree are bundled (OFL).

### Removed
- **Light and dark themes, Ghostty themes and custom themes.** 2.0 has one designed look; the accent colour is the remaining customisation. Theme settings in older `settings.toml` files are ignored, and the source, lyrics index and accent are kept.

### Notes
- The play history file moves to version 3; older files are migrated on first start.
- 1.x ("Classic") is frozen at `coda-gui-v1.0.0` and gets critical fixes only.

## [1.0.0] - 2026-11-19 (Classic)

The first release of the desktop app: local files and Navidrome in one queue, Now Playing with lyrics, genres, mixes, and light and dark themes.
