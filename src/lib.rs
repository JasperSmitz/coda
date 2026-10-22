//! Coda: a keyboard-first terminal music player.
//!
//! The library holds all non-UI logic so it can be tested directly:
//! domain models, the queue, the pure player coordinator, the playback
//! boundary (with its mpv backend) and local library discovery.

pub mod config;
pub mod library;
pub mod model;
pub mod playback;
pub mod player;
pub mod queue;
