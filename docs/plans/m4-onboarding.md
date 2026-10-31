# M4 — Onboarding: implementation brief

- **Status:** approved by owner 2026-10-31 ("do the onboarding first"). Password storage: ADR 0004.
- **Goal:** a new user can set Coda up from inside the app: an optional local music folder and an optional Navidrome connection (URL → username → password). Hand-editing the config file stays possible but is no longer needed.
- **Delivery:** two slices, each implemented, independently reviewed, fixed and committed. No push.

## Slice 1 — first-run setup

### When it runs
- `Config::load` returns `ConfigError::NoSource` **and** stdin and stdout are terminals → run the setup instead of exiting with the usage error.
- Not a terminal → keep today's usage error and exit code 2 (scripts and pipes do not change).
- Any other config error (bad TOML, permissions, partial Navidrome config, …) still fails as today. Setup only fills in what is missing; it never repairs a broken file.
- Sources given through the CLI argument, `CODA_MUSIC_DIR` or `CODA_NAVIDROME_*` count as configured, so setup does not run.

### Flow (one screen at a time, calm and compact like the main UI)
1. **Welcome:** one or two lines explaining what follows. `Enter` continues, `Esc`/`q` quits.
2. **Local music:**
   - "Do you have a local music folder?" A path input prefilled with `~/Music` if that folder exists.
   - `Enter` validates the folder (leading `~/` expanded, must exist and be a readable directory, same rules as `validate_music_dir`). Errors are shown inline and the input stays editable.
   - A visible "skip" (`Esc` or `Tab`, the developer chooses and shows the hint) moves on without a folder.
3. **Navidrome:**
   - "Do you have a Navidrome server?" `y` starts the form, `n`/`Esc` skips.
   - The form asks for URL, then username, then password. The password is masked with `•`, and pasted text goes into the field.
   - The URL is validated with the existing `parse_base_url` rules. A missing scheme gets `https://` added, with the actual URL shown before connecting.
   - On submit, Coda runs a `ping` with these credentials. A spinner/"Connecting…" shows while the UI stays responsive. Results:
     - Success: "Connected as <username>".
     - Failure: the existing short messages ("Could not reach server", "Authentication failed", "Not a Subsonic server", …), shown inline. The user can edit any field and retry, or skip Navidrome.
   - Never show or log the password, a token, or the signed URL. The server URL may appear on screen, since the user typed it, but never in the log (ADR 0003).
4. **Save and continue:**
   - Store the password in the keychain (ADR 0004), then write the config file.
   - If the keychain is unavailable or refuses, show a clear inline error. On Linux it names the Secret Service, e.g. GNOME Keyring or KWallet. The user can retry or skip Navidrome. Coda never falls back to plaintext.
   - Then continue straight into the normal player, without asking the user to restart.
5. **Both skipped:** show "Nothing to play yet. Coda will ask again next time." Exit with code 0 when a key is pressed. No config file is written.

### Config and keychain
- New `[navidrome]` key `password_keychain = true`.
  - Setting more than one of `password`, `password_command` and `password_keychain` is a config error (exit 2). `CODA_NAVIDROME_PASSWORD` still overrides all of them.
  - Resolving it reads the keychain entry, alongside `password_command` before the TUI. A missing entry, or a keychain that is unavailable or denies access, is a clear error (exit 1). It names the remedy without revealing the secret, e.g. "run coda again after removing password_keychain from <path> to set it up again" (the developer phrases this).
- Keychain entry:
  - Service `coda`. The account identifies user and server: username plus host, and port if not the default.
  - A small `secret_store` (or similarly named) module wraps the crate: get, set, delete. A test double is used only in tests; prefer the crate's own mock store over a hand-rolled trait if it fits.
  - No other module depends on the keyring crate.
- Crate:
  - Use the current `keyring` release (4.x at time of writing; check its API and feature/backends model).
  - Backends: macOS Keychain, and the Linux Secret Service, preferring a pure-Rust D-Bus backend over one that needs `libdbus` headers at build time. Document the choice and what Linux builds need.
  - Pin exact versions as the project does.
- **Writing the config file:**
  - Path: the same as for reading (`CODA_CONFIG` or the XDG default). Create the parent directory with mode 0700 if missing.
  - Write atomically as `session.rs` does: temp file in the same directory, mode 0600, `fsync`, rename.
  - Contents: `music_dir` (with the `~/` form kept if the user typed it), and `[navidrome]` with `url`, `username` and `password_keychain = true`. Add a short comment header saying the file was written by setup and may be edited.
  - If a file already exists at that path (only possible when it configures no source, e.g. comments only):
    - Keep any `scrobble` setting.
    - Only if the file contains anything other than whitespace and comments, back it up to `config.toml.bak.<secs>` before replacing it.
    - Use `toml`'s serializer or a small explicit writer. Do not add a TOML-editing crate for this.

### Structure
- Pure state machine `onboarding.rs` (screens, inputs, validation results, what to do next). Render code lives in `ui/`. Reuse `line_input` and add a masked display mode. No I/O in the state machine: it emits requests (validate folder, ping, save), which a small driver performs, the same pattern as `App` and the select loop.
- The driver runs **before** mpv is spawned, in its own TUI session, with the same terminal and panic-hook discipline as `app::run` (restored on every exit path, never `Terminal::drop` on a hung-up tty). When it finishes, `main` reloads the config with `Config::load` and continues the normal startup. A brief alternate-screen switch is acceptable. The ping uses a temporary `navidrome::Client` built from the entered values.
- Keep `app.rs` unchanged in this slice, apart from anything strictly needed.

### Tests
- **State machine:** every transition, including skip/skip → exit, folder validation errors, URL normalisation and errors, a failed ping followed by a retry, keychain failure → retry/skip, the masked password never appearing in render output, and paste going into the focused field.
- **Config:**
  - `password_keychain` parsing.
  - Mutual exclusivity of the three password keys.
  - Precedence with `CODA_NAVIDROME_PASSWORD`.
  - Resolution through the mock store, including a missing entry and an unavailable store.
- **Config writer:** round trip through `Config::load`, mode 0600, an atomic write that leaves no temp file, a backup only when needed, an existing `scrobble` kept, and a 0700 parent created.
- **Render (`TestBackend`):** each screen, inline errors, connecting state.
- **Non-terminal runs:** the CLI integration test still gets the usage error and exit 2 without a terminal.
- **Ping:** wiremock (success and auth failure). No test touches the real keychain or a real server.

### Acceptance
- Standard checks pass: fmt, clippy `-D warnings`, `test --all-targets`, build, and the ignored mpv tests.
- A PTY smoke run, with `CODA_CONFIG` and `XDG_*` pointing into a temp dir and a fake Subsonic server, goes through: skip both → exit 0; folder only; Navidrome only with a wrong password and then the right one. The mock store or a test keychain must be used, since the smoke run must not write to the owner's login keychain. If a real-keychain run is wanted, leave it to the owner as a manual step.
- README documents first-run setup and `password_keychain`. ARCHITECTURE describes the new modules.

## Slice 2 — connect, edit and disconnect from within the app
- Pressing `2` without Navidrome configured offers "Connect to Navidrome…" (the same form as slice 1). A settings entry, with the key chosen with the owner in M6 and a temporary key until then, lets the user edit the URL, username or password, or disconnect.
- Connecting or editing at runtime replaces the `navidrome::Client` in the running app: the browse cache is cleared, in-flight responses from the old client are ignored, and the scrobbler is restarted. Queued remote tracks keep their song ids and resolve against the new client.
- Disconnecting removes the keychain entry and the `[navidrome]` section, and the remote tracks in the queue fail like any unresolved remote entry.
- Only settings written by Coda (`password_keychain`) can be edited in the app. A hand-configured `password`/`password_command`, or `CODA_NAVIDROME_*` variables, show "configured outside Coda" read-only.
- A detailed brief follows after slice 1, based on what slice 1 settles.

## Out of scope
Multiple servers, OpenSubsonic API keys, editing the local music folder in the app (can come with slice 2 if cheap), localisation.

## Risks
- The terminal lifecycle running twice (setup, then player), which is covered by the same discipline and a PTY smoke run.
- Keychain prompts on macOS for an unsigned binary: an OS prompt on first read is expected and acceptable.
- Linux Secret Service availability: documented, with a clear error.
