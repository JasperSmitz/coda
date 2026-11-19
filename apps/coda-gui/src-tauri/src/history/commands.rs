//! The play history's commands: Home's rows of one source and the day's
//! picks, and an artist's most played songs. History comes from the
//! engine (a copy); resolving local keys against the library and
//! fetching from the server happen here, off the engine task.

use std::time::Duration;

use coda_core::model::{Track, TrackMetadata};
use coda_core::navidrome::AlbumSort;
use serde::Serialize;
use tauri::State;

use super::{
    DAILY_PICKS, HomeDto, HomeRows, SourceKind, TOP_TRACKS, TrackCount, daily_album,
    daily_album_dto, home_dto, local_artist_top_tracks, local_daily_picks, server_id, valid_date,
};
use crate::browse::dto::{AlbumDto, BrowseError, BrowseErrorKind};
use crate::browse::{BrowseHandle, check_id, local, track_dto};
use crate::cover::CoverService;
use crate::dto::TrackDto;
use crate::engine::{EngineGone, EngineHandle};

type Result<T> = std::result::Result<T, BrowseError>;

fn gone(_: EngineGone) -> BrowseError {
    BrowseError::new(BrowseErrorKind::Failed, "Coda is quitting")
}

/// Resolves a source's rows (local ones against the current library).
pub async fn resolve_home(
    browse: &BrowseHandle,
    source: SourceKind,
    rows: HomeRows,
) -> Result<HomeDto> {
    match source {
        SourceKind::Local => {
            browse
                .local(move |view, covers| Ok(home_dto(rows, Some(&view), covers)))
                .await
        }
        SourceKind::Navidrome => {
            browse.client()?;
            Ok(home_dto(rows, None, browse.covers()))
        }
    }
}

/// Recently played and most played contexts of `source`.
#[tauri::command]
pub async fn home_history(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
) -> Result<HomeDto> {
    let rows = engine.home_rows(source).await.map_err(gone)?;
    resolve_home(&browse, source, rows).await
}

/// The Daily Picks of `date` (the UI's local calendar day,
/// `YYYY-MM-DD`): local ones drawn with the date as the seed; Navidrome's
/// fetched (`random`) at the first ask of the day and cached in the
/// history.
#[tauri::command]
pub async fn home_daily_picks(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    date: String,
) -> Result<Vec<AlbumDto>> {
    daily_picks(&engine, &browse, source, date).await
}

pub async fn daily_picks(
    engine: &EngineHandle,
    browse: &BrowseHandle,
    source: SourceKind,
    date: String,
) -> Result<Vec<AlbumDto>> {
    if !valid_date(&date) {
        return Err(BrowseError::not_found("Date"));
    }
    match source {
        SourceKind::Local => {
            browse
                .local(move |view, covers| Ok(local_daily_picks(&view, &date, covers)))
                .await
        }
        SourceKind::Navidrome => {
            let (client, _) = browse.client()?;
            let cached = engine
                .daily_picks(date.clone(), server_id(&client))
                .await
                .map_err(gone)?;
            let albums = match cached.filter(|albums| !albums.is_empty()) {
                Some(albums) => albums,
                None => {
                    // The server id goes with the fetch, so the picks are
                    // cached under the server they came from (or dropped).
                    let (server, page) = browse
                        .fetch(|c| {
                            let server = server_id(&c);
                            async move {
                                let page = c.album_list(AlbumSort::Random, 0, DAILY_PICKS).await?;
                                Ok((server, page))
                            }
                        })
                        .await?;
                    let albums = page.albums.into_iter().map(daily_album).collect();
                    engine
                        .set_daily_picks(date, server, albums)
                        .await
                        .map_err(gone)?
                        .ok_or_else(|| {
                            BrowseError::new(
                                BrowseErrorKind::Stale,
                                "The server connection changed; reload",
                            )
                        })?
                }
            };
            Ok(albums
                .iter()
                .map(|a| daily_album_dto(a, browse.covers()))
                .collect())
        }
    }
}

/// One of an artist's most played songs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopTrackDto {
    pub track: TrackDto,
    /// Listens counted by this app.
    pub listens: u64,
}

/// "Your most played" on an artist page: up to [`TOP_TRACKS`] of the
/// artist's songs by the listens this app counted, most first; empty
/// without any. Local: `id` is the artist's library id. Navidrome: the
/// current server's songs whose artist id is `id`; a song without an
/// artist id matches when its album artist or artist is named `name`.
#[tauri::command]
pub async fn artist_most_played(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    id: String,
    name: String,
) -> Result<Vec<TopTrackDto>> {
    artist_top_tracks(&engine, &browse, source, id, name).await
}

pub async fn artist_top_tracks(
    engine: &EngineHandle,
    browse: &BrowseHandle,
    source: SourceKind,
    id: String,
    name: String,
) -> Result<Vec<TopTrackDto>> {
    match source {
        SourceKind::Local => {
            let listens = engine.local_track_listens().await.map_err(gone)?;
            browse
                .local(move |view, covers| {
                    let artist = local::artist_id(&view, &id)?;
                    Ok(
                        local_artist_top_tracks(view.library, artist, &listens, TOP_TRACKS)
                            .into_iter()
                            .map(|(track, _, listens)| TopTrackDto {
                                track: track_dto(view.library.track(track), Some(&view), covers),
                                listens,
                            })
                            .collect(),
                    )
                })
                .await
        }
        SourceKind::Navidrome => {
            browse.client()?;
            check_id(&id)?;
            let name: String = name.chars().take(1024).collect();
            let top = engine
                .navidrome_top_tracks(id, name)
                .await
                .map_err(gone)?
                .unwrap_or_default();
            Ok(top
                .into_iter()
                .filter_map(|count| navidrome_top_track(count, browse.covers()))
                .collect())
        }
    }
}

/// A counted Navidrome song as a row, from what the history kept.
fn navidrome_top_track(count: TrackCount, covers: &CoverService) -> Option<TopTrackDto> {
    let song = count.track.song?;
    let mut track = Track::navidrome(
        count.track.id,
        TrackMetadata {
            title: Some(song.title),
            artist: song.artist,
            album: song.album,
            album_artist: song.album_artist,
            track_no: song.track_no,
            disc_no: song.disc_no,
            duration: song.duration_ms.map(Duration::from_millis),
            cover_art: song.cover_art,
            ..TrackMetadata::default()
        },
    );
    track.album_id = song.album_id;
    track.artist_id = song.artist_id;
    Some(TopTrackDto {
        track: track_dto(&track, None, covers),
        listens: count.listens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::SourceDto;
    use crate::history::{HeardTrack, SongInfo};

    #[test]
    fn a_counted_navidrome_song_becomes_a_row_with_its_cover_and_links() {
        let covers = CoverService::new(None);
        let count = TrackCount {
            track: HeardTrack {
                source: SourceKind::Navidrome,
                server: Some(server_id(
                    &coda_core::navidrome::Client::new(
                        url::Url::parse("https://music.example/").unwrap(),
                        "listener".into(),
                        coda_core::navidrome::Password::new("pw".into()),
                    )
                    .unwrap(),
                )),
                id: "s1".into(),
                song: Some(SongInfo {
                    title: "Song".into(),
                    album: Some("Album".into()),
                    album_id: Some("al-1".into()),
                    artist_id: Some("ar-1".into()),
                    cover_art: Some("al-1".into()),
                    duration_ms: Some(61_000),
                    ..SongInfo::default()
                }),
            },
            listens: 12,
            last: 1,
        };
        let row = navidrome_top_track(count.clone(), &covers).unwrap();
        assert_eq!(row.listens, 12);
        assert_eq!(row.track.title, "Song");
        assert_eq!(row.track.album_id.as_deref(), Some("al-1"));
        assert_eq!(row.track.artist_id.as_deref(), Some("ar-1"));
        assert_eq!(row.track.duration_ms, Some(61_000));
        assert!(matches!(
            row.track.source,
            SourceDto::Navidrome { ref song_id, ref cover_art }
                if song_id == "s1" && cover_art.as_deref() == Some("al-1")
        ));
        // Without the song's row (never written so): nothing to show.
        let bare = TrackCount {
            track: HeardTrack {
                song: None,
                ..count.track
            },
            ..count
        };
        assert!(navidrome_top_track(bare, &covers).is_none());
    }
}
