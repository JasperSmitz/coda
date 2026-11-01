# ADR 0004 — Navidrome password in the OS keychain

- **Status:** Accepted for M4 (owner, 2026-10-31). Extends ADR 0003.
- **Context:** In-app onboarding means Coda itself must store the Navidrome password. ADR 0003 only allowed a password the user stores themselves (chmod-600 file, environment variable, or `password_command`).
- **Decision:**
  - Onboarding stores the password in the OS credential store: the macOS Keychain, or the Secret Service (GNOME Keyring, KWallet) on Linux. It goes through the `keyring` crate, under one fixed service name and an account identifying the username and server.
  - The config file then records `password_keychain = true` in `[navidrome]`, together with `url` and `username`. It never contains the password itself.
  - If no credential store is available, onboarding says so (including what to install on Linux) and lets the user skip Navidrome. There is deliberately **no** plaintext fallback written by Coda. The manual options from ADR 0003 remain for users who want them.
  - Source precedence is unchanged, with the keychain added as one more file-configured source. `CODA_NAVIDROME_PASSWORD` overrides any file source. The file may name at most one of `password`, `password_command` and `password_keychain`.
- **Alternatives:**
  - A plaintext password in a chmod-600 file written by Coda: works everywhere, but stores the password unencrypted at rest.
  - Storing the derived token and salt: still a replayable credential, so nothing gained.
  - OpenSubsonic API keys: not yet universally supported; can be added later as a revocable alternative.
- **Consequences:**
  - New dependency on `keyring` and its platform backends.
  - On Linux, a running Secret Service is required for in-app onboarding. The owner accepts that minimal or headless setups use the manual config instead.
  - Reading the keychain may show an OS prompt at startup (on macOS, the first time).
  - Tests use an in-memory credential store and never touch the real keychain.
