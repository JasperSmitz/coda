# Changelog

All notable changes to Coda are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Coda uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- The repository is now a Cargo workspace with the TUI in `crates/coda-tui` (package `coda-tui`); log targets in `coda.log` are now `coda_tui::…` instead of `coda_player::…` (adjust any `CODA_LOG=coda_player=…` filter).

## [1.0.0] - 2026-11-03

The first release: a keyboard-first terminal music player for local files and a Navidrome server, with one queue for both. Playback runs through mpv.

### Added

#### Playback
- Play, pause, seek, volume, and previous and next track, with mpv as the audio engine.
- Play in context: Enter plays the album, playlist, folder or search results from the selected track, and `s` plays them shuffled. Shuffle works on artists and folders too, and "Shuffle all" plays your whole library.
- Gapless playback: consecutive tracks play without a pause, local files and Navidrome streams alike.
- Repeat off, all or one.
- A missing or unplayable file is skipped with a message; playback goes on.

#### Queue
- One queue for local and Navidrome tracks. Navidrome tracks are marked `☁`.
- Add a track, album or playlist to the end (`a`) or play it next (`A`).
- Move, remove and shuffle upcoming entries, clear the queue, and jump to the current track.
- The queue, volume, repeat mode and visualizer setting are restored at the next launch.

#### Local library
- Browse by playlists (`.m3u` and `.m3u8`), folders, artists or albums. Artists and albums come from the tags.
- The scan runs in the background, so the player stays responsive in large libraries.
- Search by title, artist, album or path.

#### Navidrome
- Browse playlists, artists and albums, with six sort orders for albums, and search.
- Stream into the same queue as local tracks.
- Scrobble plays to Navidrome (can be turned off).
- Create, rename, reorder and delete your playlists, add songs to them, remove songs, and save the queue as a playlist.

#### Setup and settings
- A first-run setup in the terminal: an optional music folder and an optional Navidrome server, with the connection tested before saving.
- The Navidrome password is stored in the macOS Keychain or the Linux Secret Service. A password command or the config file also work.
- Connect, edit or disconnect Navidrome from within the app (`O`) while music keeps playing, or run `coda --setup` again.

#### Display
- An optional spectrum and level meter (`v`), in step with what you hear.
- Cover art of the track you hear: a real image in Ghostty and Kitty, colour blocks in other terminals. Local covers come from the tags or the folder; Navidrome covers are cached on disk.
- A Now playing screen (`f`) with a large cover, the title, the timeline and the visualizer.
- A help screen with every key (`?`).

#### Privacy
- Logs never contain passwords, tokens, stream URLs, file paths or track titles.

[Unreleased]: https://github.com/JasperSmitz/coda/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/JasperSmitz/coda/releases/tag/v1.0.0
