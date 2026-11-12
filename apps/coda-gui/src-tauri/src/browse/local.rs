//! Local library queries: plain functions over a [`LocalView`], run on a
//! blocking thread by the commands. Orders follow `coda_core`'s tag index
//! and folder tree (as Coda's local browser shows them).

use coda_core::cover::CoverKey;
use std::collections::HashSet;

use coda_core::library::search;
use coda_core::library::tags::{AlbumId, AlbumOrder, ArtistId};
use coda_core::library::tree::TrackId;
use coda_core::library::{Library, PlaylistId};
use coda_core::lyrics::index::{DEFAULT_LIMIT, Snippet};
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

pub(crate) fn first_track_cover(
    library: &Library,
    tracks: &[TrackId],
    covers: &CoverService,
) -> Option<String> {
    let path = library.track(*tracks.first()?).local_path()?;
    covers.url_for_key(CoverKey::Local(path.to_path_buf()))
}

pub(crate) fn album_dto(view: &LocalView<'_>, id: AlbumId, covers: &CoverService) -> AlbumDto {
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

pub(crate) fn artist_id(view: &LocalView<'_>, id: &str) -> Result<ArtistId, BrowseError> {
    let n = parse_local_id(id, view.generation)?;
    ((n as usize) < view.library.tags().artists().len())
        .then_some(ArtistId(n))
        .ok_or_else(|| BrowseError::not_found("Artist"))
}

pub(crate) fn album_id(view: &LocalView<'_>, id: &str) -> Result<AlbumId, BrowseError> {
    let n = parse_local_id(id, view.generation)?;
    // Album ids are dense: every album appears in each order once.
    ((n as usize) < view.library.tags().albums(AlbumOrder::Name).len())
        .then_some(AlbumId(n))
        .ok_or_else(|| BrowseError::not_found("Album"))
}

pub(crate) fn playlist_id(view: &LocalView<'_>, id: &str) -> Result<PlaylistId, BrowseError> {
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

/// The track ids a context plays (`playLocal`), in order: everything
/// below a folder in browse order, or an artist's albums A–Z, each in
/// album order (as Coda's `s`). Ids only: the engine clones just the
/// tracks it queues, and nothing is sent to the webview.
pub fn context_track_ids(
    view: &LocalView<'_>,
    context: &LocalContextDto,
) -> Result<Vec<TrackId>, BrowseError> {
    let mut ids = Vec::new();
    match context {
        LocalContextDto::Folder { path } => {
            let tree = view.library.tree();
            let dir = tree
                .dir(path)
                .ok_or_else(|| BrowseError::not_found("Folder"))?;
            tree.for_each_id(dir, |id| ids.push(id));
        }
        LocalContextDto::Artist { id } => {
            let id = artist_id(view, id)?;
            let tags = view.library.tags();
            for album in tags.artist(id).albums() {
                ids.extend_from_slice(tags.album(*album).tracks());
            }
        }
    }
    Ok(ids)
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
        editable: false,
        read_only_reason: None,
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
    let query = super::cap_query(query);
    let (artists, albums) = view.library.tags().search(&query, SEARCH_GROUP_LIMIT);
    let found = search::search(view.library.tree(), &query);
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

/// Lyrics matching `query`, leaving out every track the local search for
/// `query` matches (`Library::search_lyrics`) and the tracks in `exclude`
/// (the ids of the songs the main search listed; the mechanism Navidrome
/// will need), so no track shows twice. At
/// most [`DEFAULT_LIMIT`] in browse order, and the total. Ids from
/// another library generation make the search stale; malformed ones are
/// ignored, and only the first [`search::MAX_RESULTS`] are read.
pub fn lyrics_search(
    view: LocalView<'_>,
    query: &str,
    exclude: &[String],
    covers: &CoverService,
) -> Result<LyricsSearchDto, BrowseError> {
    let query = super::cap_query(query);
    let mut skip = HashSet::new();
    for id in exclude.iter().take(search::MAX_RESULTS) {
        match parse_local_id(id, view.generation) {
            Ok(n) if (n as usize) < view.library.len() => {
                skip.insert(TrackId(n));
            }
            Ok(_) => {}
            Err(error) if error.kind == BrowseErrorKind::Stale => return Err(error),
            Err(_) => {}
        }
    }
    let found = view.library.search_lyrics(&query, &skip, DEFAULT_LIMIT);
    Ok(LyricsSearchDto {
        matches: found
            .matches
            .into_iter()
            .map(|m| LyricsMatchDto {
                track: track_dto(view.library.track(m.key), Some(&view), covers),
                snippet: snippet_parts(&m.snippet),
            })
            .collect(),
        total: found.total,
    })
}

/// `snippet` split at its highlight boundaries.
pub(crate) fn snippet_parts(snippet: &Snippet) -> Vec<SnippetPartDto> {
    let chars: Vec<char> = snippet.text.chars().collect();
    let mut parts = Vec::new();
    let mut at = 0;
    let mut push = |range: std::ops::Range<usize>, matched: bool| {
        if range.start < range.end {
            parts.push(SnippetPartDto {
                text: chars[range].iter().collect(),
                matched,
            });
        }
    };
    for range in &snippet.highlights {
        let (start, end) = (range.start.min(chars.len()), range.end.min(chars.len()));
        if start < at {
            continue;
        }
        push(at..start, false);
        push(start..end, true);
        at = end;
    }
    push(at..chars.len(), false);
    parts
}
