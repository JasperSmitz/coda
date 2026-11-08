//! Coda: a keyboard-first terminal music player.
//!
//! The library holds the program logic so it can be tested directly:
//! domain models, the queue, the pure player coordinator, the playback
//! boundary (with its mpv backend), local library discovery, the
//! Navidrome client, session persistence, cover art (decoding, the disk
//! cache and what to show; `cover`), the Now playing screen's state
//! (`now_playing`), the visualizer's display state,
//! the first-run setup
//! (`onboarding` state, `setup` driver), the in-app settings (`settings`
//! state, performed by `app`), the OS credential store
//! (`secret_store`), and the terminal UI (`app`, `setup` and `ui`, the only
//! modules that use Ratatui or Crossterm).

pub mod app;
pub mod config;
pub mod config_writer;
pub mod cover;
pub mod library;
pub mod line_input;
pub mod model;
pub mod navidrome;
pub mod now_playing;
pub mod onboarding;
pub mod playback;
pub mod player;
pub mod queue;
pub mod resolve;
pub mod secret_store;
pub mod session;
pub mod settings;
pub mod setup;
pub mod ui;
pub mod visualizer;
