//! Coda: a keyboard-first terminal music player.
//!
//! The library holds the program logic so it can be tested directly:
//! domain models, the queue, the pure player coordinator, the playback
//! boundary (with its mpv backend), local library discovery, the
//! Navidrome client, session persistence, and the terminal UI (`app` and
//! `ui`, the only modules that use Ratatui or Crossterm).

pub mod app;
pub mod config;
pub mod library;
pub mod line_input;
pub mod model;
pub mod navidrome;
pub mod playback;
pub mod player;
pub mod queue;
pub mod resolve;
pub mod session;
pub mod ui;
