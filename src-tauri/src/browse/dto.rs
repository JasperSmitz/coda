//! Browse DTOs (camelCase JSON), mirrored in `src/lib/types.ts` (the
//! "Browse" block). Like the engine's DTOs they never carry credentials,
//! stream URLs or file paths.
//!
//! Ids are opaque strings. Local ids carry the library generation
//! (`"<generation>:<n>"`), so an id from before a rescan is recognised as
//! stale rather than naming another item; Navidrome ids are the server's.

use serde::{Deserialize, Serialize};

use crate::dto::TrackDto;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDto {
    pub id: String,
    pub name: String,
    pub album_count: Option<u32>,
    /// Local only: tracks under this artist.
    pub track_count: Option<u32>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDto {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub year: Option<u32>,
    pub track_count: Option<u32>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
}

/// Navidrome's warning data for a playlist whose server count and served
/// entries differ (`coda_core::navidrome::Hidden`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HiddenDto {
    None,
    /// The server stores this many entries it did not return.
    Some {
        count: usize,
    },
    /// The counts disagree the other way; hidden entries are possible.
    Suspect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub track_count: Option<u32>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
    /// Navidrome: the owner, when the server says.
    pub owner: Option<String>,
    /// Navidrome: the server marks it read-only (another user's, smart).
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetailDto {
    pub artist: ArtistDto,
    pub albums: Vec<AlbumDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetailDto {
    pub album: AlbumDto,
    pub tracks: Vec<TrackDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetailDto {
    pub playlist: PlaylistDto,
    pub tracks: Vec<TrackDto>,
    /// Local: entries that matched no scanned track.
    pub missing: usize,
    /// Only the start was read: `(kept, total)` for Navidrome; local
    /// files give `kept == total == tracks.len()` (the total is unknown).
    pub truncated: Option<(usize, usize)>,
    pub hidden: HiddenDto,
    /// What to tell the user about `missing`, `truncated` and `hidden`.
    pub warning: Option<String>,
}

/// One folder level: sub-folders by name, then its own tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    /// Folder names from the music root (empty: the root).
    pub path: Vec<String>,
    pub dirs: Vec<String>,
    pub tracks: Vec<TrackDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPageDto {
    pub albums: Vec<AlbumDto>,
    /// Where the next page starts; `None` after the last one.
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDto {
    pub artists: Vec<ArtistDto>,
    pub albums: Vec<AlbumDto>,
    pub tracks: Vec<TrackDto>,
    /// Every matching track, also those beyond `tracks` (local only; the
    /// server's count is unknown, so `tracks.len()` there).
    pub total_tracks: usize,
}

/// The order of the local Albums list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalAlbumOrder {
    Name,
    Artist,
}

/// `getAlbumList2`'s orders (`coda_core::navidrome::AlbumSort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlbumSortDto {
    Name,
    Artist,
    Newest,
    Frequent,
    Recent,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowseErrorKind {
    /// The source is not configured (no music folder, no server).
    NotConfigured,
    /// The id is from an older library or server connection: reload.
    Stale,
    NotFound,
    /// The server request failed (network, HTTP, auth, decode).
    Failed,
}

/// A browse command's error. `message` is for display (never credentials).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct BrowseError {
    pub kind: BrowseErrorKind,
    pub message: String,
}

impl BrowseError {
    pub fn new(kind: BrowseErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn no_library() -> Self {
        Self::new(
            BrowseErrorKind::NotConfigured,
            "No music folder is configured",
        )
    }

    pub fn no_server() -> Self {
        Self::new(
            BrowseErrorKind::NotConfigured,
            "No Navidrome server is configured",
        )
    }

    pub fn stale() -> Self {
        Self::new(
            BrowseErrorKind::Stale,
            "The library changed since this view was loaded",
        )
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(BrowseErrorKind::NotFound, format!("{what} not found"))
    }
}
