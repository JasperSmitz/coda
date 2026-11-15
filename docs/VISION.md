# Coda — Vision

## What it is
Coda is a music player for a personal collection: local files and a Navidrome server, played from one queue. It comes as a keyboard-first terminal player and a desktop app, both built on one shared engine.

Coda began as a deliberately small TUI, and that goal has been met. The aim now is a player that is a pleasure to live with: richer ways to find and play your music, while staying calm, fast and coherent. Features are welcome when they make listening better; they are not added to tick boxes.

## User experience
- Browse by artist, album, genre, playlist or folder, and play in context: in order or shuffled.
- **Mixes:** say what you're in the mood for, such as "indie rock and prog rock" or "King Crimson, Yes and Genesis", and Coda plays a balanced blend from both sources. A mix is a saved recipe, not a fixed track list, so new music joins it automatically, and it keeps playing for as long as you listen.
- See one queue, whether a track is local or on Navidrome. The queue is temporary; playlists and mixes are what last.
- Control playback without leaving what you're doing: the keyboard in the terminal; the mouse, keyboard and media keys in the desktop app.
- See what's playing: cover art, a now-playing view and lyrics (from tags, sidecar `.lrc` files and Navidrome, never online lyrics providers), and search your library by lyrics.
- Stay responsive during scans and network operations, and handle missing files, disconnected servers and playback failures gracefully.

## Guiding principles
1. **Music first:** reliable playback outweighs visual effects.
2. **Tasteful, not bare:** each feature earns its place, is finished rather than half-done, and works well with no configuration. Fewer good features beat many rough ones.
3. **For enjoying a library, not maintaining one:** Coda plays the collection as its owner has tagged and organised it. Fixing tags, fetching cover art, renaming or moving files and syncing to a server are the owner's business, done with their own tools. Coda reads tags and never writes them; it does not fetch metadata, and it makes no assumptions about how a library is organised (such as a genre hierarchy). Playlists and mixes are Coda's own; they are part of listening.
4. **Your collection is the source:** Coda works from your files and your server. Core features never depend on outside services or accounts.
5. **Source-independent playback:** local and Navidrome tracks behave the same in the queue, mixes and shuffle.
6. **One engine, two frontends:** behaviour lives in the shared core; each frontend is native to its medium (keyboard-first in the terminal, a proper desktop app in the GUI).
7. **Understandable architecture:** the simplest design that supports what is being built now; no speculative frameworks.

## Direction
In scope and planned, roughly in this order:
- **Genres:** read the genres a track has (local tags and Navidrome; a track can have several, as separate tags or one separated list), browse by genre, shuffle a genre. Genres are taken as tagged: a broad genre and a subgenre are simply two genres.
- **Mixes:** recipes made of genres and artists (other ingredients later), balanced across their ingredients and topped up as you listen. Mixing local and Navidrome sources includes recognising the same song in both, so it plays once.
- **Endless play:** when the queue runs out, keep going from the current context or mix.

Already shipped beyond the original scope: lyrics and lyrics search (with a local cache of Navidrome lyrics, ADR 0007), cover art, gapless playback, the visualizer, media keys and the desktop app.

## Not in scope
- Library maintenance: tag editing, metadata or cover lookups, file organising and deduplicating the library's files (principle 3).
- Equalizers and other audio processing. mpv plays the audio, and Coda does not decode or stream audio itself.
- Streaming services other than Navidrome, outside recommendation services, automatic downloads and offline caching.
- A daemon mode and Windows support.
- A persistent library database. Coda scans into memory; ADR 0007's lyrics cache is the one exception.
- HEART integration (postponed indefinitely).

Moving something into scope is the owner's decision, recorded here with the date and a plan in `docs/plans/`. The scope was last widened on 2026-11-15, from a minimal TUI to a feature-rich but tasteful player.
