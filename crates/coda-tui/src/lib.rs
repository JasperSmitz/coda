//! Coda: a keyboard-first terminal music player.
//!
//! The terminal frontend of Coda: the TUI's event loop (`app`), the
//! first-run setup driver (`setup`) and the drawing (`ui`), the only
//! modules that use Ratatui or Crossterm. The engine (queue, player, mpv
//! playback, library, Navidrome, session, settings and setup state) is
//! `coda_core`.

pub mod app;
pub mod setup;
pub mod ui;
