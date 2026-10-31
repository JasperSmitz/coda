//! Terminal UI: key mapping, rendering, the help overlay and the setup
//! screens. Together with `app.rs` and `setup.rs`, the only code that uses
//! Ratatui or Crossterm.

pub mod help;
pub mod keymap;
pub mod onboarding;
pub mod render;
