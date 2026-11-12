//! Browse commands: async, read-only, returning DTOs or a [`BrowseError`].
//! They never touch `Player`; playing goes through the `intent` command.

use tauri::State;

use super::BrowseHandle;
use super::dto::*;
use super::local;
use crate::dto::TrackDto;

type Result<T> = std::result::Result<T, BrowseError>;

// ----- local --------------------------------------------------------------

#[tauri::command]
pub async fn local_artists(browse: State<'_, BrowseHandle>) -> Result<Vec<ArtistDto>> {
    browse.local(local::artists).await
}

#[tauri::command]
pub async fn local_albums(
    browse: State<'_, BrowseHandle>,
    order: LocalAlbumOrder,
) -> Result<Vec<AlbumDto>> {
    browse
        .local(move |view, covers| local::albums(view, order, covers))
        .await
}

#[tauri::command]
pub async fn local_artist(browse: State<'_, BrowseHandle>, id: String) -> Result<ArtistDetailDto> {
    browse
        .local(move |view, covers| local::artist(view, &id, covers))
        .await
}

#[tauri::command]
pub async fn local_album(browse: State<'_, BrowseHandle>, id: String) -> Result<AlbumDetailDto> {
    browse
        .local(move |view, covers| local::album(view, &id, covers))
        .await
}

#[tauri::command]
pub async fn local_folder(browse: State<'_, BrowseHandle>, path: Vec<String>) -> Result<FolderDto> {
    browse
        .local(move |view, covers| local::folder(view, path, covers))
        .await
}

#[tauri::command]
pub async fn local_playlists(browse: State<'_, BrowseHandle>) -> Result<Vec<PlaylistDto>> {
    browse.local(local::playlists).await
}

#[tauri::command]
pub async fn local_playlist(
    browse: State<'_, BrowseHandle>,
    id: String,
) -> Result<PlaylistDetailDto> {
    browse
        .local(move |view, covers| local::playlist(view, &id, covers))
        .await
}

#[tauri::command]
pub async fn local_search(browse: State<'_, BrowseHandle>, query: String) -> Result<SearchDto> {
    browse
        .local(move |view, covers| local::search(view, &query, covers))
        .await
}

/// Lyrics matches, separate from [`local_search`] so the main results
/// never wait for it. `exclude`: the song ids the main search returned.
#[tauri::command]
pub async fn local_lyrics_search(
    browse: State<'_, BrowseHandle>,
    query: String,
    exclude: Vec<String>,
) -> Result<LyricsSearchDto> {
    browse
        .local(move |view, covers| local::lyrics_search(view, &query, &exclude, covers))
        .await
}

// ----- Navidrome ----------------------------------------------------------

#[tauri::command]
pub async fn navidrome_artists(browse: State<'_, BrowseHandle>) -> Result<Vec<ArtistDto>> {
    browse.navidrome_artists().await
}

#[tauri::command]
pub async fn navidrome_artist(
    browse: State<'_, BrowseHandle>,
    id: String,
) -> Result<Vec<AlbumDto>> {
    browse.navidrome_artist(id).await
}

#[tauri::command]
pub async fn navidrome_artist_tracks(
    browse: State<'_, BrowseHandle>,
    id: String,
) -> Result<Vec<TrackDto>> {
    browse.navidrome_artist_tracks(id).await
}

#[tauri::command]
pub async fn navidrome_album(
    browse: State<'_, BrowseHandle>,
    id: String,
) -> Result<AlbumDetailDto> {
    browse.navidrome_album(id).await
}

#[tauri::command]
pub async fn navidrome_albums(
    browse: State<'_, BrowseHandle>,
    sort: AlbumSortDto,
    offset: usize,
) -> Result<AlbumPageDto> {
    browse.navidrome_albums(sort, offset).await
}

#[tauri::command]
pub async fn navidrome_playlists(browse: State<'_, BrowseHandle>) -> Result<Vec<PlaylistDto>> {
    browse.navidrome_playlists().await
}

#[tauri::command]
pub async fn navidrome_playlist(
    browse: State<'_, BrowseHandle>,
    id: String,
) -> Result<PlaylistDetailDto> {
    browse.navidrome_playlist(id).await
}

#[tauri::command]
pub async fn navidrome_random_songs(browse: State<'_, BrowseHandle>) -> Result<Vec<TrackDto>> {
    browse.navidrome_random_songs().await
}

#[tauri::command]
pub async fn navidrome_search(browse: State<'_, BrowseHandle>, query: String) -> Result<SearchDto> {
    browse.navidrome_search(query).await
}

/// Lyrics matches from the server's lyrics cache, separate from
/// [`navidrome_search`] so the main results never wait for it. `exclude`:
/// the song ids the main search returned.
#[tauri::command]
pub async fn navidrome_lyrics_search(
    browse: State<'_, BrowseHandle>,
    query: String,
    exclude: Vec<String>,
) -> Result<LyricsSearchDto> {
    browse.navidrome_lyrics_search(query, exclude).await
}
