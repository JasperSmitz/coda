//! Lyrics for Now Playing's lyrics mode: the `now_playing_lyrics` command.
//!
//! The parsing and reading live in `coda_core::lyrics` (tags, a sidecar
//! `.lrc`) and `coda_core::navidrome::Client::lyrics`; this module only
//! finds the track and maps the result to a DTO. It runs on the command's
//! own task and a blocking thread, never on the engine actor: a local id
//! is resolved through the [`BrowseHandle`]'s read view (a stale
//! generation gives `None`, like a track the library does not hold), and a
//! Navidrome song is fetched with the current client within the browse
//! fetch limit. Nothing is cached here (the frontend keeps a few results).

use std::path::PathBuf;

use coda_core::lyrics::{self, Lyrics};
use coda_core::model::TrackSource;
use serde::Serialize;
use tauri::State;

use crate::browse::{self, BrowseHandle, check_id};
use crate::dto::{SourceDto, TrackRef};

/// The lyrics of one track (camelCase JSON, mirrored in `src/lib/types.ts`).
/// When `timed`, every line has a `start_ms` and the lines are sorted by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDto {
    pub timed: bool,
    pub lines: Vec<LyricLineDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLineDto {
    pub start_ms: Option<u64>,
    pub text: String,
}

impl LyricsDto {
    /// `None` for lyrics with no text at all (nothing worth showing).
    fn from_lyrics(lyrics: Lyrics) -> Option<Self> {
        if lyrics.is_blank() {
            return None;
        }
        Some(Self {
            timed: lyrics.timed,
            lines: lyrics
                .lines
                .into_iter()
                .map(|line| LyricLineDto {
                    start_ms: line
                        .start
                        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                    text: line.text,
                })
                .collect(),
        })
    }
}

/// The lyrics of `track` (the playing track), `None` when it has none or
/// cannot be found. An error is a failed read or request, which the view
/// shows as "Couldn't load lyrics".
#[tauri::command]
pub async fn now_playing_lyrics(
    browse: State<'_, BrowseHandle>,
    track: TrackRef,
) -> Result<Option<LyricsDto>, String> {
    lyrics_for(&browse, track).await
}

/// [`now_playing_lyrics`] without Tauri's state wrapper (for tests).
pub async fn lyrics_for(
    browse: &BrowseHandle,
    track: TrackRef,
) -> Result<Option<LyricsDto>, String> {
    let lyrics = match track.source {
        SourceDto::Local { id } => {
            let Some(id) = id else { return Ok(None) };
            local_lyrics(browse, id).await?
        }
        SourceDto::Navidrome { song_id, .. } => {
            if check_id(&song_id).is_err() {
                return Ok(None);
            }
            let artist = track.artist.map(|a| cap(&a));
            let title = cap(track.title.as_deref().unwrap_or_default());
            let result = browse
                .fetch_raw(|client| async move {
                    client.lyrics(&song_id, artist.as_deref(), &title).await
                })
                .await
                .map_err(|error| {
                    tracing::debug!(error = %error.message, "lyrics: no usable server");
                    error.message
                })?;
            result.map_err(|error| {
                tracing::debug!(detail = %error.detail(), "lyrics request failed");
                error.to_string()
            })?
        }
    };
    Ok(lyrics.and_then(LyricsDto::from_lyrics))
}

/// The lyrics of the local track `id`: its path is looked up under the
/// library lock, the file is read after the lock is released.
async fn local_lyrics(browse: &BrowseHandle, id: String) -> Result<Option<Lyrics>, String> {
    let handle = browse.clone();
    tokio::task::spawn_blocking(move || {
        let Some(path) = local_path(&handle, &id) else {
            return Ok(None);
        };
        lyrics::read_local(&path).map_err(|error| {
            // The error never names the path; the id is enough to follow.
            tracing::debug!(%error, id = %id, "reading local lyrics failed");
            "The file could not be read".to_string()
        })
    })
    .await
    .map_err(|_| "Reading the lyrics failed".to_string())?
}

/// The file behind a local id of the current library generation; `None`
/// for a stale or unknown id, or without a library.
fn local_path(browse: &BrowseHandle, id: &str) -> Option<PathBuf> {
    browse.read_library(|view| {
        let view = view?;
        let n = browse::parse_local_id(id, view.generation).ok()?;
        match &view.library.tree().tracks().get(n as usize)?.source {
            TrackSource::Local { path } => Some(path.clone()),
            TrackSource::Navidrome { .. } => None,
        }
    })
}

/// Artist and title from the UI, bounded like other input.
fn cap(text: &str) -> String {
    text.chars().take(browse::MAX_INPUT_CHARS).collect()
}

#[cfg(test)]
mod tests;
