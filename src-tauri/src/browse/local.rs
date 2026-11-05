//! Local library queries: plain functions over a [`LocalView`], run on a
//! blocking thread by the commands. Orders follow `coda_core`'s tag index
//! and folder tree (as Coda's local browser shows them).

use coda_core::cover::CoverKey;
use coda_core::library::search;
use coda_core::library::tags::{AlbumId, AlbumOrder, ArtistId};
use coda_core::library::tree::TrackId;
use coda_core::library::{Library, PlaylistId};
use coda_core::model::thousands;

use super::dto::*;
use super::{LocalView, local_id, parse_local_id, track_dto};
use crate::cover::CoverService;
use crate::dto::TrackDto;

/// Artists and albums kept per search (tracks follow
/// `coda_core::library::search::MAX_RESULTS`).
pub const SEARCH_GROUP_LIMIT: usize = 50;

fn u32_count(n: usize) -> Option<u32> {
    Some(u32::try_from(n).unwrap_or(u32::MAX))
}

fn first_track_cover(
    library: &Library,
    tracks: &[TrackId],
    covers: &CoverService,
) -> Option<String> {
    let path = library.track(*tracks.first()?).local_path()?;
    covers.url_for_key(CoverKey::Local(path.to_path_buf()))
}

fn album_dto(view: &LocalView<'_>, id: AlbumId, covers: &CoverService) -> AlbumDto {
    let tags = view.library.tags();
    let album = tags.album(id);
    let artist = tags.artist(album.artist());
    AlbumDto {
        id: local_id(view.generation, id.0),
        name: album.name().to_owned(),
        artist: Some(artist.name().to_owned()),
        artist_id: Some(local_id(view.generation, artist.id().0)),
        year: None,
        track_count: u32_count(album.tracks().len()),
        duration_ms: album.duration().map(|d| d.as_millis() as u64),
        cover_url: first_track_cover(view.library, album.tracks(), covers),
    }
}

fn artist_dto(view: &LocalView<'_>, id: ArtistId, covers: &CoverService) -> ArtistDto {
    let tags = view.library.tags();
    let artist = tags.artist(id);
    let cover_url = artist
        .albums()
        .first()
        .and_then(|a| first_track_cover(view.library, tags.album(*a).tracks(), covers));
    ArtistDto {
        id: local_id(view.generation, id.0),
        name: artist.name().to_owned(),
        album_count: u32_count(artist.albums().len()),
        track_count: u32_count(artist.track_count()),
        cover_url,
    }
}

fn tracks_dto(view: &LocalView<'_>, ids: &[TrackId], covers: &CoverService) -> Vec<TrackDto> {
    ids.iter()
        .map(|id| track_dto(view.library.track(*id), Some(view), covers))
        .collect()
}

fn artist_id(view: &LocalView<'_>, id: &str) -> Result<ArtistId, BrowseError> {
    let n = parse_local_id(id, view.generation)?;
    ((n as usize) < view.library.tags().artists().len())
        .then_some(ArtistId(n))
        .ok_or_else(|| BrowseError::not_found("Artist"))
}

fn album_id(view: &LocalView<'_>, id: &str) -> Result<AlbumId, BrowseError> {
    let n = parse_local_id(id, view.generation)?;
    // Album ids are dense: every album appears in each order once.
    ((n as usize) < view.library.tags().albums(AlbumOrder::Name).len())
        .then_some(AlbumId(n))
        .ok_or_else(|| BrowseError::not_found("Album"))
}

fn playlist_id(view: &LocalView<'_>, id: &str) -> Result<PlaylistId, BrowseError> {
    let n = parse_local_id(id, view.generation)?;
    ((n as usize) < view.library.playlists().len())
        .then_some(PlaylistId(n))
        .ok_or_else(|| BrowseError::not_found("Playlist"))
}

/// Every artist, A–Z (unknown last).
pub fn artists(view: LocalView<'_>, covers: &CoverService) -> Result<Vec<ArtistDto>, BrowseError> {
    let tags = view.library.tags();
    Ok(tags
        .artists()
        .iter()
        .map(|id| artist_dto(&view, *id, covers))
        .collect())
}

/// Every album in `order`.
pub fn albums(
    view: LocalView<'_>,
    order: LocalAlbumOrder,
    covers: &CoverService,
) -> Result<Vec<AlbumDto>, BrowseError> {
    let order = match order {
        LocalAlbumOrder::Name => AlbumOrder::Name,
        LocalAlbumOrder::Artist => AlbumOrder::Artist,
    };
    Ok(view
        .library
        .tags()
        .albums(order)
        .iter()
        .map(|id| album_dto(&view, *id, covers))
        .collect())
}

/// An artist and their albums, A–Z.
pub fn artist(
    view: LocalView<'_>,
    id: &str,
    covers: &CoverService,
) -> Result<ArtistDetailDto, BrowseError> {
    let id = artist_id(&view, id)?;
    let albums = view
        .library
        .tags()
        .artist(id)
        .albums()
        .iter()
        .map(|a| album_dto(&view, *a, covers))
        .collect();
    Ok(ArtistDetailDto {
        artist: artist_dto(&view, id, covers),
        albums,
    })
}

/// Every track of an artist: each album A–Z, in album order (what `s`
/// plays, as in Coda).
pub fn artist_tracks(
    view: LocalView<'_>,
    id: &str,
    covers: &CoverService,
) -> Result<Vec<TrackDto>, BrowseError> {
    let id = artist_id(&view, id)?;
    let tags = view.library.tags();
    let ids: Vec<TrackId> = tags
        .artist(id)
        .albums()
        .iter()
        .flat_map(|a| tags.album(*a).tracks().iter().copied())
        .collect();
    Ok(tracks_dto(&view, &ids, covers))
}

/// An album and its tracks (disc, track number, title, path).
pub fn album(
    view: LocalView<'_>,
    id: &str,
    covers: &CoverService,
) -> Result<AlbumDetailDto, BrowseError> {
    let id = album_id(&view, id)?;
    let tracks = tracks_dto(&view, view.library.tags().album(id).tracks(), covers);
    Ok(AlbumDetailDto {
        album: album_dto(&view, id, covers),
        tracks,
    })
}

/// One folder level. `path`: folder names from the music root.
pub fn folder(
    view: LocalView<'_>,
    path: Vec<String>,
    covers: &CoverService,
) -> Result<FolderDto, BrowseError> {
    let dir = view
        .library
        .tree()
        .dir(&path)
        .ok_or_else(|| BrowseError::not_found("Folder"))?;
    Ok(FolderDto {
        dirs: dir.dirs().iter().map(|d| d.name().to_owned()).collect(),
        tracks: tracks_dto(&view, dir.tracks(), covers),
        path,
    })
}

/// Every track below a folder, in browse order (what `s` plays).
pub fn folder_tracks(
    view: LocalView<'_>,
    path: Vec<String>,
    covers: &CoverService,
) -> Result<Vec<TrackDto>, BrowseError> {
    let tree = view.library.tree();
    let dir = tree
        .dir(&path)
        .ok_or_else(|| BrowseError::not_found("Folder"))?;
    let mut ids = Vec::new();
    tree.for_each_id(dir, |id| ids.push(id));
    Ok(tracks_dto(&view, &ids, covers))
}

fn playlist_dto(view: &LocalView<'_>, id: PlaylistId, covers: &CoverService) -> PlaylistDto {
    let playlist = view.library.playlist(id);
    let durations: Option<u64> = playlist
        .tracks
        .iter()
        .map(|t| {
            view.library
                .track(*t)
                .duration
                .map(|d| d.as_millis() as u64)
        })
        .sum();
    PlaylistDto {
        id: local_id(view.generation, id.0),
        name: playlist.name.clone(),
        track_count: u32_count(playlist.tracks.len()),
        duration_ms: durations.filter(|_| !playlist.tracks.is_empty()),
        cover_url: first_track_cover(view.library, &playlist.tracks, covers),
        owner: None,
        readonly: false,
    }
}

/// Every `.m3u` playlist, A–Z.
pub fn playlists(
    view: LocalView<'_>,
    covers: &CoverService,
) -> Result<Vec<PlaylistDto>, BrowseError> {
    Ok(view
        .library
        .playlists()
        .iter()
        .map(|id| playlist_dto(&view, *id, covers))
        .collect())
}

/// A playlist's matched tracks in file order, duplicates kept.
pub fn playlist(
    view: LocalView<'_>,
    id: &str,
    covers: &CoverService,
) -> Result<PlaylistDetailDto, BrowseError> {
    let id = playlist_id(&view, id)?;
    let playlist = view.library.playlist(id);
    let tracks = tracks_dto(&view, &playlist.tracks, covers);
    let mut notes = Vec::new();
    if playlist.missing > 0 {
        notes.push(format!(
            "{} not found in the library",
            if playlist.missing == 1 {
                "1 entry".to_owned()
            } else {
                format!("{} entries", thousands(playlist.missing))
            }
        ));
    }
    if playlist.cut {
        notes.push("only the start of the file was read".to_owned());
    }
    let warning = (!notes.is_empty()).then(|| {
        let mut text = notes.join("; ");
        if let Some(first) = text.get(..1) {
            text = first.to_uppercase() + &text[1..];
        }
        text
    });
    Ok(PlaylistDetailDto {
        playlist: playlist_dto(&view, id, covers),
        missing: playlist.missing,
        truncated: playlist.cut.then_some((tracks.len(), tracks.len())),
        tracks,
        hidden: HiddenDto::None,
        warning,
    })
}

/// Artists and albums by name, tracks by title, artist, album or path.
pub fn search(
    view: LocalView<'_>,
    query: &str,
    covers: &CoverService,
) -> Result<SearchDto, BrowseError> {
    let (artists, albums) = view.library.tags().search(query, SEARCH_GROUP_LIMIT);
    let found = search::search(view.library.tree(), query);
    let (tracks, total_tracks) = match found {
        Some(found) => (
            found
                .tracks
                .iter()
                .map(|t| track_dto(t, Some(&view), covers))
                .collect(),
            found.total,
        ),
        None => (Vec::new(), 0),
    };
    Ok(SearchDto {
        artists: artists
            .into_iter()
            .map(|a| artist_dto(&view, a, covers))
            .collect(),
        albums: albums
            .into_iter()
            .map(|a| album_dto(&view, a, covers))
            .collect(),
        tracks,
        total_tracks,
    })
}
