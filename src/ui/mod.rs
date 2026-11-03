//! Terminal UI: key mapping, rendering, cover art (detection, Kitty and
//! half blocks), the Now playing screen, the help and Settings overlays
//! and the setup screens. Together with `app.rs` and `setup.rs`, the only code that uses
//! Ratatui or Crossterm.

pub mod cover;
pub mod help;
pub mod keymap;
pub mod now_playing;
pub mod onboarding;
pub mod render;
pub mod settings;
