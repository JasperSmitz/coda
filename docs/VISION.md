# Coda — Vision

## What it is
Coda is a keyboard-first Rust TUI music player with a calm, compact interface. It plays local music files and streams from a personal Navidrome server, sharing one queue. It works by itself; a HEART music module was once planned and is postponed indefinitely.

## User experience
- Launch `coda`, browse a library, and play in context: an album, playlist or artist, in order or shuffled.
- Treat the queue as temporary: add to it on the fly; keep lasting collections as playlists.
- See a single queue regardless of whether a track originates locally or on Navidrome.
- Control play/pause, next/previous, seek and volume without leaving the terminal.
- Keep the interface responsive during file scans and network operations.
- Gracefully handle missing files, disconnected servers and playback failures.

## Guiding principles
1. **Music first:** reliable playback outweighs visual effects.
2. **Keyboard first:** discoverable and ergonomic terminal controls.
3. **Source-independent playback:** consistent queue and playback behavior.
4. **Portable core:** no dependency on HEART or a particular terminal layout.
5. **Small and understandable:** choose the simplest architecture that supports the current milestone.

## Not in the initial scope
Lyrics, smart playlists, equalizer, library deduplication, automatic downloads, offline caching, MPRIS, daemon mode, music recommendations, and HEART integration. Revisit them only when justified. Gapless playback is proposed as M8 and cover art as M9; endless play and context-aware autoplay are noted for later.
