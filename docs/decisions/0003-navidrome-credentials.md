# ADR 0003 — Navidrome configuration and credentials

- **Status:** Accepted for M2
- **Context:** M2 needs a server URL, username and password. M1 had no config file. Credentials must never be committed, logged or leaked through stream URLs handed to mpv.
- **Decision:**
  - An optional TOML config file (`$XDG_CONFIG_HOME/coda/config.toml`, default `~/.config/coda/config.toml`, `CODA_CONFIG` overrides) holds `music_dir` and a `[navidrome]` section. Env vars (`CODA_NAVIDROME_URL/USER/PASSWORD`) override it.
  - The password comes from the file (only if the file is not group/other-accessible), the env var, or `password_command` (argv, no shell, run once before the TUI).
  - Subsonic salted-token auth with a fresh salt per request; the plaintext password is never sent.
  - Password and signed URLs live in redacting wrapper types. HTTP errors are stripped of their URL. mpv receives the stream URL only over its private IPC socket and does not inherit the `CODA_NAVIDROME_*` environment.
- **Alternatives:** env vars only (simple, but passwords in shell profiles and inherited by children); OS keychain (better storage, but platform-specific crates and headless-Linux friction — `password_command` covers `pass`, `security find-generic-password`, `secret-tool` without a dependency); OpenSubsonic API keys (not universally supported yet).
- **Consequences:** a `toml` and `reqwest` dependency; a token-bearing URL exists in memory and in mpv for the duration of a stream. A leaked token+salt pair is replayable on Subsonic servers, which is why redaction is tested rather than assumed.
- **Implementation notes (M2 slice 1):**
  - The `reqwest` `rustls` feature brings in aws-lc-rs and the OS trust store (`rustls-platform-verifier`), so self-hosted CAs installed in the system work. It needs a C compiler to build, as ring would; there is no OpenSSL dependency.
  - Redirects are not followed: reqwest copies the previous URL, query included, into `Referer`, which would send the signed token to the redirect target.
  - A config file that sets `password` must not be group/other-accessible. One that sets `password_command` must not be group/other-writable.
  - The local log may record error kinds, but never URLs, hostnames, tokens or response bodies.
