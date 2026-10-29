# M2 — Navidrome integration: implementation brief

- **Status:** approved by owner 2026-10-29 (autonomous session authorizing M1–M3).
- **Scope:** one Navidrome (Subsonic/OpenSubsonic) server alongside the local library; browse, search and stream remote songs into the same queue.
- **Delivery:** two slices, each implemented by the developer, independently reviewed, fixed, re-verified, then committed by the architect. No push.

## 1. Decisions

| Area | Decision |
|---|---|
| Config | Optional TOML file `$XDG_CONFIG_HOME/coda/config.toml`, default `~/.config/coda/config.toml` on both platforms; `CODA_CONFIG` overrides the path. Env vars override file values. See ADR 0003. |
| Sources | At least one of: music directory, Navidrome. Neither → usage error, exit 2. Music dir precedence: CLI arg > `CODA_MUSIC_DIR` > `music_dir` in file. |
| Auth | Subsonic token auth: `t = md5(password + salt)`, fresh random salt per request, `v=1.16.1`, `c=coda`, `f=json`. No plaintext `p=` parameter ever. |
| HTTP | `reqwest` (rustls, no OpenSSL, `default-features = false`), one shared client, connect timeout 5 s, request timeout 15 s, response bodies capped (8 MiB) while reading. |
| Domain | `TrackSource::Navidrome { song_id: String }`. Single server, so no server id. Local and remote tracks share `Track`, `Queue` and `Player` unchanged. |
| Credentials boundary | `Player` and the mpv backend never see credentials. A resolution step in the app layer turns a track source into a playback target immediately before the command reaches the backend: local path, or a freshly signed stream URL. |
| Secrets in memory | Password and stream URLs are wrapped in types whose `Debug`/`Display` redact (`"<redacted>"`). Only the mpv protocol encoder reads the raw URL. |
| Logs/errors | Never log or display URLs that carry auth (API calls or streams), passwords, or tokens. `reqwest::Error` contains the URL in its `Display`, so every one is passed through `.without_url()` before it is stored, logged or shown. |
| Tests | Mock HTTP via `wiremock` (dev-dependency). Real-mpv streaming test serves a generated WAV from a local mock server (`#[ignore]`). No test touches the owner's server. |

## 2. Config (`config.rs`)

```toml
music_dir = "~/Music"            # optional; leading "~/" expanded using $HOME

[navidrome]                      # optional section
url = "https://music.example.org"   # http or https; may include a path prefix
username = "me"
password = "…"                   # either this …
password_command = ["pass", "show", "navidrome"]   # … or this (argv, no shell)
```

- Env overrides: `CODA_NAVIDROME_URL`, `CODA_NAVIDROME_USER`, `CODA_NAVIDROME_PASSWORD`. Navidrome is configured when url, username and a password source are all present after merging; a partial configuration is a clear config error (exit 2) naming the missing field, never the values.
- Missing config file is fine. Unparseable file → error with path and line (exit 2). Unknown keys → error (catches typos).
- If the file contains `password` and its mode grants any group/other permission (`mode & 0o077 != 0`), refuse to start with an error suggesting `chmod 600 <path>` (exit 1).
- `password_command` runs once at startup before the TUI: spawned directly (no shell), stdin/stderr inherited (pinentry/gpg prompts work), stdout captured, 30 s timeout then kill, non-zero exit or empty output is an error. One trailing newline (`\n` or `\r\n`) is stripped. The command's stdout is never logged.
- URL validation: parse with `url`; scheme must be `http`/`https`; no query, fragment or embedded userinfo. Normalise so the base path ends in `/` and endpoints join as `rest/<name>` (reverse-proxy prefixes keep working). Plain `http` to a non-loopback host is allowed but logged once as a warning (without the URL).
- mpv child: `env_remove` `CODA_NAVIDROME_PASSWORD` (and the other `CODA_NAVIDROME_*` vars) so they are not inherited.

## 3. Subsonic client (`navidrome/`, new module; no Ratatui, no mpv)

- `navidrome::client` — `Client` (base URL, username, password, `reqwest::Client`). Async methods: `ping`, `artists` (getArtists, flatten index → `Vec<Artist>` sorted as served), `artist(id)` (getArtist → albums), `album(id)` (getAlbum → songs as `Track`s), `search(query)` (search3; artistCount 20, albumCount 20, songCount 50). Plus sync `stream_url(song_id) -> StreamUrl` (signing is pure apart from the salt).
- `navidrome::api` — serde types for `subsonic-response`. `status: "failed"` → `ApiError::Server { code, message }`, with code 40/41 (wrong credentials / token auth not supported) mapped to an `Auth` variant. Unknown fields ignored. Missing optional fields tolerated.
- Error enum (`thiserror`): `Network` (timeout/connection, URL stripped), `Http(status)`, `Auth`, `Server{code,message}`, `Decode`, `TooLarge`. User-facing text is short and contains no URL.
- Untrusted data: all strings go through the same capping as tags (`MAX_METADATA_CHARS`); durations via `try_from_secs_f64`/integer seconds with saturation; ids are kept as opaque strings (capped; an empty id makes the item skipped). Server error messages are capped too.
- `Track::navidrome(song_id, metadata)` builds remote tracks with title fallback `"Unknown title"` (no file stem available). `Track::local_path()` returns `None` for remote tracks; add a `is_remote()`/source accessor as needed.

## 4. Playback target and mpv

- The backend command `Load` carries a target: `File(PathBuf)` or `Stream(StreamUrl)`. Exact type names are the developer's choice; the rule is that `Player` emits loads in terms of the queue entry's `TrackSource` and the app resolves them, and the backend never sees `TrackSource::Navidrome`.
- Resolution failure (a remote track with no Navidrome configured, e.g. a stale entry) must reach the player as `TrackEnded { load, reason: Error(..) }` for that load, so the queue advances exactly as for a missing file. Cover this with a test.
- Streams skip the regular-file pre-check; only `StreamUrl`s built by the client (http/https) can be loaded.
- Add `--network-timeout=15` to mpv argv. Keep `--ytdl=no --access-references=no`.
- Seeking a stream works through mpv's cache/range requests; no transcoding parameters are sent (server defaults).
- Event-ordering guarantees from M1 are unchanged; existing tests keep passing with the new `Load` shape.

## 5. Slice 1 — config, client, unified playback

**Delivers:** config file + env + credentials handling, `navidrome` module, `TrackSource::Navidrome`, load resolution, mpv stream support. `main.rs` builds the client when configured and passes it to `app::run`; the app does a background `ping` after the TUI starts and shows the result in the status line ("Navidrome connected" / short error). The TUI otherwise stays as in M1; with only Navidrome configured it opens with an empty local library and the status message. No browsing UI yet.

**Acceptance:**
- Config precedence, validation, permission check, password_command (success, failure, timeout, empty output) and env stripping for mpv tested.
- Client tested against `wiremock`: auth parameters present (`u`, `t`, `s`, `v`, `c`, `f`), `t == md5(password + s)`, fresh salt per request, no `p` parameter; ping ok/failed/auth error; getArtists/getArtist/getAlbum/search3 parsing including missing fields and hostile values (huge strings, negative/huge durations, empty ids); HTTP 500; non-JSON body; body over the cap; timeout (short timeout configured in test).
- A test proves errors from a failing request (e.g. connection refused) do not contain the password, token, salt or the query string; `format!("{:?}")` of the client, password and stream URL is redacted.
- Path-prefix base URLs join correctly.
- Resolution: local → file target; remote → stream URL for that song id; remote without client → `TrackEnded{Error}` and the queue advances.
- `#[ignore]` real-mpv: a WAV served by wiremock at a stream URL produces `Started` then `TrackEnded{Eof}`; a 404 stream produces `TrackEnded{Error}`.
- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all-targets`, `build`, and ignored mpv tests pass.

## 6. Slice 2 — remote browsing, search and TUI

**Delivers:**
- Library panel has two sources: Local and Navidrome (only when configured). `1` / `2` select the source (focusing the Library panel) from either panel. The panel title shows the active source and, for Navidrome, the connection state.
- Navidrome browse: Artists → Albums → Songs. `Enter` opens an artist/album or plays a song now (same insert-after-current rule as local); `h`/`Backspace` goes up; `a` enqueues a song, an album (all songs in order) or an artist (all albums in served order, songs in order). Albums/artists not yet fetched are fetched in the background and enqueued when they arrive; the user can keep working.
- Search: `/` in the Library panel while the Navidrome source is active opens a one-line input in the panel; `Enter` submits search3, `Esc` cancels. Results show sections Artists, Albums, Songs with the same Enter/`a` behaviour; `h` returns to the view before the search. Input is single-line, capped at 256 chars, control characters ignored.
- `r` refreshes the current Navidrome view (refetches; clears that cache entry).
- Async model: a request id per fetch; responses arrive on a channel into the select loop; a response is applied to the cache always, and to the view only if it is still the awaited one. Views show "Loading…" and errors inline ("Could not reach server", "Authentication failed", …). Session-only in-memory cache of fetched lists. No request ever blocks the loop; concurrent requests are bounded (e.g. a semaphore of 4).
- Queue panel and now-playing bar mark remote tracks (a short marker such as `☁` or `[N]`; pick one that renders in common terminals and test its width handling).
- Help overlay updated.

**Acceptance:**
- App-state tests (no network): navigation stack, stale responses ignored for the view but cached, loading/error states, enqueue of album/artist after fetch completes in order, search input editing and cancel, source switching preserves each source's position, `1`/`2` when Navidrome is not configured shows a message.
- Wiremock-backed test of the fetch task (request → response event) including error mapping.
- Render tests (`TestBackend`): Navidrome artists, loading, error, search input and results, remote marker in queue.
- Manual smoke against a local mock or real server is optional and only with the owner's own configuration; automated tests never need it.
- All standard checks pass.

## 7. Out of scope (M2)
Multiple servers, playlists, starring/favourites, scrobbling, cover art, transcoding options, offline caching, local search, gapless/prefetch, persistence of the remote cache, OpenSubsonic API-key auth. M3 may pick some of these up.

## 8. Risks
- Token-in-URL leakage through error messages, `Debug` derives or mpv messages — covered by redaction types and tests.
- Large libraries: getArtists returns everything in one response; the body cap must accommodate realistic sizes (8 MiB ≈ tens of thousands of artists).
- Stream seeks depend on server range support; acceptable.
