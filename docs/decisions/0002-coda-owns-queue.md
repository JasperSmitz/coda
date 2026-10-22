# ADR 0002 — Coda owns the queue; mpv plays one track at a time

- **Status:** Accepted for M1
- **Context:** Both Coda and mpv can hold a playlist. Two queues means two sources of truth, harder stale-event handling, and a queue that would have to change shape when Navidrome tracks join in M2.
- **Decision:** Coda's queue is authoritative. The mpv backend holds at most one playlist entry, loaded with `loadfile … replace`. Track-end events are matched to Coda load ids through mpv's `playlist_entry_id`. Coda advances only on EOF or error for the current load.
- **Alternatives:** Mirror the queue into mpv's playlist (gapless and prefetch for free, but two-way synchronisation on every edit). Let mpv own the queue (simplest, but source identity and queue semantics would leak into the backend).
- **Consequences:** Short gaps between tracks; mpv's gapless and prefetch features are unused. If this matters, add a one-track lookahead (append the next entry to mpv) while Coda stays authoritative. Queue and advance logic stay pure and unit-testable.
