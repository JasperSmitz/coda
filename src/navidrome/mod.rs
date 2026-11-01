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
pub mod edit;
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
    /// Who may edit it; see [`Access`].
    pub access: Access,
}

/// Who owns a playlist, as the server reports it: the `owner` username
/// and OpenSubsonic's `readonly` flag (Navidrome sets it for other users'
/// playlists and for smart playlists).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// Untrusted, capped like other metadata. `None` when not sent.
    pub owner: Option<String>,
    pub readonly: bool,
}

impl Access {
    /// `true` if `username` may change the playlist: it owns it (compared
    /// case-insensitively, as Navidrome usernames are) and the server does
    /// not mark it read-only. An unknown owner is never editable.
    pub fn editable_by(&self, username: &str) -> bool {
        !self.readonly
            && !username.is_empty()
            && self
                .owner
                .as_deref()
                .is_some_and(|owner| owner.to_lowercase() == username.to_lowercase())
    }
}

/// The songs of a playlist (`getPlaylist`), in playlist order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistSongs {
    /// The songs with a usable id, from the first
    /// [`api::MAX_PLAYLIST_ENTRIES`] entries.
    pub tracks: Vec<crate::model::Track>,
    /// How many entries the server sent in all, usable or not.
    pub served: usize,
    /// Who may edit it.
    pub access: Access,
    /// The server's `songCount`. Navidrome counts every stored entry,
    /// including the ones `getPlaylist` hides (missing files, libraries
    /// the user cannot access); `None` when not sent.
    pub song_count: Option<u32>,
}

/// Entries of a playlist the server stores but does not return, which a
/// replace of the list would remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// None, or the server does not send `songCount` (nothing to compare).
    None,
    /// `songCount` exceeds the entries returned by this many.
    Some(usize),
    /// `songCount` is below the entries returned: the counts disagree, so
    /// hidden entries cannot be ruled out.
    Suspect,
}

impl Hidden {
    /// An upper bound for comparisons: a confirmation of `self` covers any
    /// state whose bound is not higher.
    pub fn bound(self) -> usize {
        match self {
            Self::None => 0,
            Self::Some(n) => n,
            Self::Suspect => usize::MAX,
        }
    }
}

impl PlaylistSongs {
    /// Entries the server stores but did not return; see [`Hidden`].
    pub fn hidden_entries(&self) -> Hidden {
        let Some(count) = self.song_count else {
            return Hidden::None;
        };
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        match count.cmp(&self.served) {
            std::cmp::Ordering::Equal => Hidden::None,
            std::cmp::Ordering::Greater => Hidden::Some(count - self.served),
            std::cmp::Ordering::Less => Hidden::Suspect,
        }
    }

    /// `true` if every entry the server sent is held, in order: none was
    /// skipped (no usable id) and the list was not cut at the cap. Only
    /// then do positions match the server's, so only then may entries be
    /// removed by index or the order replaced.
    pub fn is_complete(&self) -> bool {
        self.served == self.tracks.len()
    }

    /// The song ids in playlist order.
    pub fn song_ids(&self) -> Vec<String> {
        self.tracks
            .iter()
            .filter_map(|t| t.navidrome_id().map(str::to_owned))
            .collect()
    }

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
