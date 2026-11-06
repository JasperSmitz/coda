//! The Home view's commands: the history rows of one source, and the
//! day's picks. History comes from the engine (a copy); resolving local
//! contexts against the library and fetching from the server happen here,
//! off the engine task.

use coda_core::navidrome::AlbumSort;
use tauri::State;

use super::{
    DAILY_PICKS, HomeDto, HomeRows, SourceKind, daily_album, daily_album_dto, home_dto,
    local_daily_picks, valid_date,
};
use crate::browse::BrowseHandle;
use crate::browse::dto::{AlbumDto, BrowseError, BrowseErrorKind};
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
            let cached = engine.daily_picks(date.clone()).await.map_err(gone)?;
            let albums = match cached.filter(|albums| !albums.is_empty()) {
                Some(albums) => albums,
                None => {
                    let page =
                        browse
                            .fetch(|c| async move {
                                c.album_list(AlbumSort::Random, 0, DAILY_PICKS).await
                            })
                            .await?;
                    let albums = page.albums.into_iter().map(daily_album).collect();
                    engine.set_daily_picks(date, albums).await.map_err(gone)?
                }
            };
            Ok(albums
                .iter()
                .map(|a| daily_album_dto(a, browse.covers()))
                .collect())
        }
    }
}
