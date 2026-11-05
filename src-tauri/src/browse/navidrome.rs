//! Navidrome queries through the current client ([`BrowseHandle::fetch`]:
//! bounded concurrency, stale results dropped). Nothing is cached here;
//! the webview keeps what it shows.

use coda_core::model::Track;
use coda_core::model::thousands;
use coda_core::navidrome::browse::MAX_ALBUMS;
use coda_core::navidrome::browse::read_only_reason;
use coda_core::navidrome::{Access, Album, AlbumSort, Artist, Hidden, Playlist};
use tokio::task::JoinSet;

use super::dto::*;
use super::{BrowseHandle, remote_cover, track_dto};
use crate::cover::CoverService;
use crate::dto::TrackDto;

const HIDDEN_WHY: &str = "(missing files or other libraries)";

/// Albums per page of the infinite album grid.
pub const ALBUM_PAGE: usize = 100;

impl From<AlbumSortDto> for AlbumSort {
    fn from(sort: AlbumSortDto) -> Self {
        match sort {
            AlbumSortDto::Name => Self::Name,
            AlbumSortDto::Artist => Self::Artist,
            AlbumSortDto::Newest => Self::Newest,
            AlbumSortDto::Frequent => Self::Frequent,
            AlbumSortDto::Recent => Self::Recent,
            AlbumSortDto::Random => Self::Random,
        }
    }
}

fn artist_dto(artist: &Artist, covers: &CoverService) -> ArtistDto {
    ArtistDto {
        id: artist.id.clone(),
        name: artist.name.clone(),
        album_count: artist.album_count,
        track_count: None,
        cover_url: remote_cover(covers, artist.cover_art.as_ref()),
    }
}

fn album_dto(album: &Album, covers: &CoverService) -> AlbumDto {
    AlbumDto {
        id: album.id.clone(),
        name: album.name.clone(),
        artist: album.artist.clone(),
        artist_id: album.artist_id.clone(),
        year: album.year,
        track_count: album.song_count,
        duration_ms: album.duration.map(|d| d.as_millis() as u64),
        cover_url: remote_cover(covers, album.cover_art.as_ref()),
    }
}

/// `(editable, read_only_reason)` of `access` for `username`.
fn editability(access: &Access, username: &str) -> (bool, Option<String>) {
    if access.editable_by(username) {
        (true, None)
    } else {
        (false, Some(read_only_reason(access, username).to_owned()))
    }
}

fn playlist_dto(playlist: &Playlist, covers: &CoverService, username: &str) -> PlaylistDto {
    let (editable, read_only_reason) = editability(&playlist.access, username);
    PlaylistDto {
        id: playlist.id.clone(),
        name: playlist.name.clone(),
        track_count: playlist.song_count,
        duration_ms: playlist.duration.map(|d| d.as_millis() as u64),
        cover_url: remote_cover(covers, playlist.cover_art.as_ref()),
        owner: playlist.access.owner.clone(),
        readonly: playlist.access.readonly,
        editable,
        read_only_reason,
    }
}

fn tracks_dto(tracks: &[Track], covers: &CoverService) -> Vec<TrackDto> {
    tracks.iter().map(|t| track_dto(t, None, covers)).collect()
}

impl BrowseHandle {
    /// Every artist, in the server's (alphabetical) index order.
    pub async fn navidrome_artists(&self) -> Result<Vec<ArtistDto>, BrowseError> {
        let artists = self.fetch(|c| async move { c.artists().await }).await?;
        Ok(artists
            .iter()
            .map(|a| artist_dto(a, self.covers()))
            .collect())
    }

    /// An artist's albums, as served.
    pub async fn navidrome_artist(&self, id: String) -> Result<Vec<AlbumDto>, BrowseError> {
        let albums = self.fetch(|c| async move { c.artist(&id).await }).await?;
        Ok(albums.iter().map(|a| album_dto(a, self.covers())).collect())
    }

    /// Every track of an artist: their albums in served order, each in
    /// album order (what `s` plays). Albums load in parallel within the
    /// fetch limit.
    pub async fn navidrome_artist_tracks(&self, id: String) -> Result<Vec<TrackDto>, BrowseError> {
        let albums = self.fetch(|c| async move { c.artist(&id).await }).await?;
        let mut set = JoinSet::new();
        for (i, album) in albums.into_iter().enumerate() {
            let handle = self.clone();
            set.spawn(async move {
                let id = album.id;
                (i, handle.fetch(|c| async move { c.album(&id).await }).await)
            });
        }
        let mut parts: Vec<(usize, Vec<Track>)> = Vec::new();
        while let Some(joined) = set.join_next().await {
            let (i, result) = joined
                .map_err(|_| BrowseError::new(BrowseErrorKind::Failed, "A request failed"))?;
            parts.push((i, result?));
        }
        parts.sort_by_key(|(i, _)| *i);
        Ok(parts
            .iter()
            .flat_map(|(_, tracks)| tracks_dto(tracks, self.covers()))
            .collect())
    }

    /// An album's tracks in served order, with a header built from them
    /// (`getAlbum` gives no year here; the UI keeps the one it listed).
    pub async fn navidrome_album(&self, id: String) -> Result<AlbumDetailDto, BrowseError> {
        let album_id = id.clone();
        let tracks = self.fetch(|c| async move { c.album(&id).await }).await?;
        let first = tracks.first();
        let durations: Option<u64> = tracks
            .iter()
            .map(|t| t.duration.map(|d| d.as_millis() as u64))
            .sum();
        let album = AlbumDto {
            id: album_id,
            name: first
                .and_then(|t| t.album.clone())
                .unwrap_or_else(|| coda_core::navidrome::UNKNOWN_ALBUM.to_owned()),
            artist: first.and_then(|t| t.album_artist.clone().or_else(|| t.artist.clone())),
            artist_id: first.and_then(|t| t.artist_id.clone()),
            year: None,
            track_count: Some(u32::try_from(tracks.len()).unwrap_or(u32::MAX)),
            duration_ms: durations.filter(|_| !tracks.is_empty()),
            cover_url: remote_cover(self.covers(), first.and_then(|t| t.cover_art.as_ref())),
        };
        Ok(AlbumDetailDto {
            album,
            tracks: tracks_dto(&tracks, self.covers()),
        })
    }

    /// One page of albums in `sort` order from `offset` (infinite scroll;
    /// stops at Coda's `MAX_ALBUMS`).
    pub async fn navidrome_albums(
        &self,
        sort: AlbumSortDto,
        offset: usize,
    ) -> Result<AlbumPageDto, BrowseError> {
        if offset >= MAX_ALBUMS {
            return Ok(AlbumPageDto {
                albums: Vec::new(),
                next_offset: None,
            });
        }
        let size = ALBUM_PAGE.min(MAX_ALBUMS - offset);
        let sort = AlbumSort::from(sort);
        let page = self
            .fetch(|c| async move { c.album_list(sort, offset, size).await })
            .await?;
        let next = offset + page.served;
        Ok(AlbumPageDto {
            albums: page
                .albums
                .iter()
                .map(|a| album_dto(a, self.covers()))
                .collect(),
            next_offset: (page.served >= size && next < MAX_ALBUMS).then_some(next),
        })
    }

    pub async fn navidrome_playlists(&self) -> Result<Vec<PlaylistDto>, BrowseError> {
        let (playlists, username) = self
            .fetch(|c| async move {
                let username = c.username().to_owned();
                c.playlists().await.map(|p| (p, username))
            })
            .await?;
        Ok(playlists
            .iter()
            .map(|p| playlist_dto(p, self.covers(), &username))
            .collect())
    }

    /// A playlist's songs in order, with the hidden-entries warning data
    /// (`Hidden`) Coda shows before edits.
    pub async fn navidrome_playlist(&self, id: String) -> Result<PlaylistDetailDto, BrowseError> {
        let playlist_id = id.clone();
        let (songs, username) = self
            .fetch(|c| async move {
                let username = c.username().to_owned();
                c.playlist(&id).await.map(|s| (s, username))
            })
            .await?;
        let (editable, read_only_reason) = editability(&songs.access, &username);
        let hidden = songs.hidden_entries();
        let mut notes = Vec::new();
        if let Some((kept, total)) = songs.truncated() {
            notes.push(format!(
                "Showing the first {} of {} entries",
                thousands(kept),
                thousands(total)
            ));
        }
        match hidden {
            Hidden::None => {}
            Hidden::Some(1) => notes.push(format!("Navidrome hides 1 entry {HIDDEN_WHY}")),
            Hidden::Some(n) => notes.push(format!(
                "Navidrome hides {} entries {HIDDEN_WHY}",
                thousands(n)
            )),
            Hidden::Suspect => notes.push(format!("Navidrome may hide entries {HIDDEN_WHY}")),
        }
        let skipped = songs
            .served
            .min(coda_core::navidrome::api::MAX_PLAYLIST_ENTRIES)
            .saturating_sub(songs.tracks.len());
        let durations: Option<u64> = songs
            .tracks
            .iter()
            .map(|t| t.duration.map(|d| d.as_millis() as u64))
            .sum();
        let playlist = PlaylistDto {
            id: playlist_id,
            name: String::new(),
            track_count: songs.song_count,
            duration_ms: durations.filter(|_| !songs.tracks.is_empty()),
            cover_url: None,
            owner: songs.access.owner.clone(),
            readonly: songs.access.readonly,
            editable,
            read_only_reason,
        };
        Ok(PlaylistDetailDto {
            playlist,
            tracks: tracks_dto(&songs.tracks, self.covers()),
            missing: skipped,
            truncated: songs.truncated(),
            hidden: match hidden {
                Hidden::None => HiddenDto::None,
                Hidden::Some(count) => HiddenDto::Some { count },
                Hidden::Suspect => HiddenDto::Suspect,
            },
            warning: (!notes.is_empty()).then(|| notes.join(". ")),
        })
    }

    /// A fresh set of random songs ("Shuffle all", as in Coda).
    pub async fn navidrome_random_songs(&self) -> Result<Vec<TrackDto>, BrowseError> {
        let tracks = self
            .fetch(|c| async move { c.random_songs().await })
            .await?;
        Ok(tracks_dto(&tracks, self.covers()))
    }

    /// `search3`: artists, albums and songs.
    pub async fn navidrome_search(&self, query: String) -> Result<SearchDto, BrowseError> {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return Ok(SearchDto {
                artists: Vec::new(),
                albums: Vec::new(),
                tracks: Vec::new(),
                total_tracks: 0,
            });
        }
        let found = self
            .fetch(|c| async move { c.search(&query).await })
            .await?;
        Ok(SearchDto {
            artists: found
                .artists
                .iter()
                .map(|a| artist_dto(a, self.covers()))
                .collect(),
            albums: found
                .albums
                .iter()
                .map(|a| album_dto(a, self.covers()))
                .collect(),
            total_tracks: found.tracks.len(),
            tracks: tracks_dto(&found.tracks, self.covers()),
        })
    }
}
