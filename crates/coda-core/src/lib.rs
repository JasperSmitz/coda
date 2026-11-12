//! Coda's shared, UI-free music engine, used by both frontends: the
//! terminal player (`coda-tui`) and the desktop app (`coda-gui`).
//!
//! It holds domain models, the queue, the pure player coordinator, the
//! playback boundary (with its mpv backend), local library discovery, the
//! Navidrome client, session persistence, configuration, cover art
//! (decoding, the disk cache and what to show; `cover`), lyrics (the LRC
//! parser and a local file's lyrics; `lyrics`), the Now playing
//! state (`now_playing`), the visualizer's display state, first-run setup
//! state (`onboarding`) and its UI-free actions (`setup_actions`), the
//! settings state (`settings`), text-field editing (`line_input`), logging
//! setup and the OS credential store
//! (`secret_store`). Nothing here depends on a terminal or GUI toolkit;
//! each frontend draws this state its own way.

pub mod config;
pub mod config_writer;
pub mod cover;
pub mod library;
pub mod line_input;
pub mod logging;
pub mod lyrics;
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
pub mod setup_actions;
pub mod visualizer;
