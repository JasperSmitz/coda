# Coda — Vision

## What it is
Coda is a keyboard-first Rust TUI music player with a calm, compact interface. It plays local music files and, later, streams from a personal Navidrome server. It works by itself and may eventually provide the music module inside HEART.

## User experience
- Launch `coda`, browse a library, choose a track, and listen.
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
Artwork, lyrics, smart playlists, equalizer, library deduplication, automatic downloads, offline caching, MPRIS, daemon mode, music recommendations, and HEART integration. Revisit them only when justified.
