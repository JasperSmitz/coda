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
- In the desktop app, show the playing track's lyrics in Now Playing (lyrics mode), from the file's tags or a sidecar `.lrc` and from Navidrome; timed lyrics follow the song. No online lyrics providers.

## Guiding principles
1. **Music first:** reliable playback outweighs visual effects.
2. **Keyboard first:** discoverable and ergonomic terminal controls.
3. **Source-independent playback:** consistent queue and playback behavior.
4. **Portable core:** no dependency on HEART or a particular terminal layout.
5. **Small and understandable:** choose the simplest architecture that supports the current milestone.

## Not in the initial scope
Smart playlists, equalizer, library deduplication, automatic downloads, offline caching, MPRIS, daemon mode, music recommendations, and HEART integration. Revisit them only when justified. Lyrics were moved into scope on 2026-11-12 (lyrics in the GUI's Now Playing, from tags and Navidrome; `docs/plans/lyrics.md`); a TUI lyrics screen is not planned. Cover art is M9 and a full-screen now-playing view is proposed as M10; endless play and context-aware autoplay are noted for later.
