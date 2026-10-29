//! Navidrome (Subsonic/OpenSubsonic) source: an HTTP client for browsing
//! and searching, and signed stream URLs for playback. No Ratatui, no mpv.
//!
//! Every server response is untrusted: strings are capped like tags,
//! numbers are converted without panicking, and items without a usable id
//! are skipped. Credentials never appear in `Debug`/`Display` output or in
//! errors.

pub mod api;
pub mod browse;
pub mod client;

use std::time::Duration;

pub use client::{Client, Error, Password};

/// An artist as listed by `getArtists` or `search3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub album_count: Option<u32>,
}

/// An album as listed by `getArtist` or `search3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub year: Option<u32>,
    pub song_count: Option<u32>,
    pub duration: Option<Duration>,
}

/// Results of a `search3` query, each list in served order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<crate::model::Track>,
}

/// Name shown for an artist whose name is missing or blank.
pub const UNKNOWN_ARTIST: &str = "Unknown artist";
/// Name shown for an album whose name is missing or blank.
pub const UNKNOWN_ALBUM: &str = "Unknown album";
