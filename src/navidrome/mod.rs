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
pub mod scrobble;

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

/// A playlist as listed by `getPlaylists`. Its songs are fetched with
/// `getPlaylist`, as [`crate::model::Track`]s in playlist order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    /// As reported by the server (saturated at `u32::MAX`).
    pub song_count: Option<u32>,
    pub duration: Option<Duration>,
}

/// The songs of a playlist (`getPlaylist`), in playlist order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistSongs {
    /// The songs with a usable id, from the first
    /// [`api::MAX_PLAYLIST_ENTRIES`] entries.
    pub tracks: Vec<crate::model::Track>,
    /// How many entries the server sent in all, usable or not.
    pub served: usize,
}

impl PlaylistSongs {
    /// `(kept, total)` entries when the playlist was cut at
    /// [`api::MAX_PLAYLIST_ENTRIES`].
    pub fn truncated(&self) -> Option<(usize, usize)> {
        (self.served > api::MAX_PLAYLIST_ENTRIES)
            .then_some((api::MAX_PLAYLIST_ENTRIES, self.served))
    }

    /// `" (first 10,000 of 12,345)"` when cut, else empty.
    pub fn truncation_note(&self) -> String {
        match self.truncated() {
            Some((kept, total)) => format!(
                " (first {} of {})",
                crate::model::thousands(kept),
                crate::model::thousands(total)
            ),
            None => String::new(),
        }
    }
}

/// The order of a `getAlbumList2` listing (its `type` parameter). A closed
/// set: nothing else is ever sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AlbumSort {
    /// `alphabeticalByName`.
    #[default]
    Name,
    /// `alphabeticalByArtist`.
    Artist,
    /// `newest`: recently added.
    Newest,
    /// `frequent`: most played.
    Frequent,
    /// `recent`: recently played.
    Recent,
    /// `random`: a new shuffle on every request, so never cached.
    Random,
}

impl AlbumSort {
    /// Every sort, in the order `o` cycles through them.
    pub const ALL: [AlbumSort; 6] = [
        Self::Name,
        Self::Artist,
        Self::Newest,
        Self::Frequent,
        Self::Recent,
        Self::Random,
    ];

    /// The `type` parameter of `getAlbumList2`.
    pub fn param(self) -> &'static str {
        match self {
            Self::Name => "alphabeticalByName",
            Self::Artist => "alphabeticalByArtist",
            Self::Newest => "newest",
            Self::Frequent => "frequent",
            Self::Recent => "recent",
            Self::Random => "random",
        }
    }

    /// Short label for titles.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "A–Z",
            Self::Artist => "by artist",
            Self::Newest => "newest",
            Self::Frequent => "most played",
            Self::Recent => "recently played",
            Self::Random => "random",
        }
    }

    /// The next sort for `o`, wrapping around.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// One page of `getAlbumList2`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumPage {
    /// The albums with a usable id, in served order.
    pub albums: Vec<Album>,
    /// How many list items the server sent, usable or not (at most the
    /// requested size): the next page starts this much further on, and a
    /// page with fewer than the requested size is the last one.
    pub served: usize,
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
