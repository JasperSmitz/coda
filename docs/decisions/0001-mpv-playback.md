# ADR 0001 — Use mpv for initial audio playback

- **Status:** Accepted for MVP
- **Context:** Coda needs dependable decoding, seeking, local playback and HTTP stream support without building an audio engine.
- **Decision:** Control an external mpv player through its JSON IPC interface from Rust. Keep a narrow playback boundary that doesn't expose mpv details to the domain or TUI.
- **Alternatives:** Rodio/Symphonia/CPAL for a Rust-native pipeline. Attractive longer-term, but adds playback and streaming complexity to the first milestone.
- **Consequences:** mpv must be installed; the app must manage the child process, IPC connection, state sync and cleanup. The backend may be replaced if real requirements justify it.
