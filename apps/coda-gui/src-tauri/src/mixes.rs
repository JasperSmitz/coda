//! Mixes (M12, `docs/plans/mixes.md`): the saved mixes file's commands
//! and the mix editor's searches. Playing a mix is an engine intent
//! (`playMix`, `playQuickMix`; `engine/mix.rs`).
//!
//! - **Store:** `list_mixes`, `add_mix`, `update_mix` and `delete_mix`
//!   call `coda_core::mix::store` on a blocking thread. Every change
//!   re-reads the shared file (`$XDG_DATA_HOME/coda/mixes.json`, ADR 0008),
//!   so the TUI's changes are never overwritten. A Navidrome mix belongs
//!   to the current server (its `server_id`, never a URL or user name).
//! - **Search:** the current source's genres, artists and albums by name,
//!   as ingredients: local ones from the library's `TagIndex` (by name, the
//!   way a local mix resolves); Navidrome genres from `getGenres` (the
//!   server's exact value), artists and albums from `search3` (id plus
//!   name; an album with its album artist, as the resolver looks it up).

use std::path::PathBuf;

use coda_core::mix::store::{self, LoadOutcome, StoreError};
use coda_core::mix::{Ingredient, Mix, MixId, MixSource};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::browse::BrowseHandle;
use crate::browse::dto::{BrowseError, BrowseErrorKind};
use crate::history::SourceKind;

/// Most genres a search returns.
pub const MAX_GENRE_RESULTS: usize = 200;
/// Most artists or albums a local search returns.
pub const MAX_NAME_RESULTS: usize = 50;

/// The mixes file (`None`: no data directory, mixes cannot be kept).
#[derive(Debug, Clone, Default)]
pub struct MixesFile(pub Option<PathBuf>);

// ----- DTOs ------------------------------------------------------------------

/// One ingredient, as the editor shows and sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IngredientDto {
    /// Local: the genre's name. Navidrome: the server's exact value.
    Genre { name: String },
    /// `id`: the Navidrome artist id (none for local).
    Artist {
        name: String,
        #[serde(default)]
        id: Option<String>,
    },
    /// `artist`: the album artist (Navidrome: as `search3` gives it).
    Album {
        artist: String,
        name: String,
        #[serde(default)]
        id: Option<String>,
    },
}

impl From<IngredientDto> for Ingredient {
    fn from(dto: IngredientDto) -> Self {
        match dto {
            IngredientDto::Genre { name } => Ingredient::Genre { name },
            IngredientDto::Artist { name, id } => Ingredient::Artist { name, id },
            IngredientDto::Album { artist, name, id } => Ingredient::Album { artist, name, id },
        }
    }
}

impl From<&Ingredient> for IngredientDto {
    fn from(ingredient: &Ingredient) -> Self {
        match ingredient.clone() {
            Ingredient::Genre { name } => IngredientDto::Genre { name },
            Ingredient::Artist { name, id } => IngredientDto::Artist { name, id },
            Ingredient::Album { artist, name, id } => IngredientDto::Album { artist, name, id },
        }
    }
}

/// A saved mix. Its source is the one it was listed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MixDto {
    pub id: String,
    pub name: String,
    pub source: SourceKind,
    pub ingredients: Vec<IngredientDto>,
}

impl MixDto {
    pub fn of(mix: &Mix) -> Self {
        Self {
            id: mix.id.to_string(),
            name: mix.name.clone(),
            source: match mix.source {
                MixSource::Local => SourceKind::Local,
                MixSource::Navidrome { .. } => SourceKind::Navidrome,
            },
            ingredients: mix.ingredients.iter().map(IngredientDto::from).collect(),
        }
    }
}

/// A recipe from the editor: to save (`add_mix`, `update_mix`) or to play
/// as a quick mix (the `playQuickMix` intent). A Navidrome mix is made
/// for the current server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixInputDto {
    pub name: String,
    pub source: SourceKind,
    pub ingredients: Vec<IngredientDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MixErrorKind {
    /// The recipe was refused (no name, no or too many ingredients, …).
    Invalid,
    /// The mix no longer exists.
    NotFound,
    /// The saved mix is of another source than the change (another
    /// server or account since the editor opened it): left as it is.
    OtherSource,
    /// There are already 500 mixes.
    Full,
    /// The file was written by a newer Coda: left alone.
    Newer,
    /// No music folder or server for the source, or no data directory.
    NotConfigured,
    /// Reading or writing the file failed.
    Failed,
}

/// A mixes command's error; `message` is for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct MixErrorDto {
    pub kind: MixErrorKind,
    pub message: String,
}

impl MixErrorDto {
    fn new(kind: MixErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn no_file() -> Self {
        Self::new(
            MixErrorKind::NotConfigured,
            "Mixes cannot be saved: no data folder (set HOME or XDG_DATA_HOME)",
        )
    }
}

impl From<StoreError> for MixErrorDto {
    fn from(error: StoreError) -> Self {
        let kind = match &error {
            StoreError::Invalid(_) => MixErrorKind::Invalid,
            StoreError::NotFound => MixErrorKind::NotFound,
            StoreError::OtherSource => MixErrorKind::OtherSource,
            StoreError::Exists | StoreError::NotKept | StoreError::TooLarge => MixErrorKind::Failed,
            StoreError::Full => MixErrorKind::Full,
            StoreError::Newer => MixErrorKind::Newer,
            StoreError::Io(_) => MixErrorKind::Failed,
        };
        if let StoreError::Io(io) = &error {
            tracing::warn!(error = %io.kind(), "mixes file change failed");
        }
        Self::new(kind, capitalized(&error.to_string()))
    }
}

impl From<BrowseError> for MixErrorDto {
    fn from(error: BrowseError) -> Self {
        let kind = match error.kind {
            BrowseErrorKind::NotConfigured => MixErrorKind::NotConfigured,
            _ => MixErrorKind::Failed,
        };
        Self::new(kind, error.message)
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ----- the store ---------------------------------------------------------------

type MixResult<T> = Result<T, MixErrorDto>;

/// The mix source for `source` now: local, or the current server's.
pub fn mix_source(browse: &BrowseHandle, source: SourceKind) -> Result<MixSource, BrowseError> {
    match source {
        SourceKind::Local => Ok(MixSource::Local),
        SourceKind::Navidrome => {
            let (client, _) = browse.client()?;
            Ok(MixSource::Navidrome {
                server: coda_core::navidrome::server_id(&client),
            })
        }
    }
}

/// A recipe from the editor as a [`Mix`] with `id`, for the current
/// server when it is a Navidrome one. Checked by the store (and by
/// `Mix::validated`).
pub fn mix_from_input(
    browse: &BrowseHandle,
    id: MixId,
    input: MixInputDto,
) -> Result<Mix, MixErrorDto> {
    let source = mix_source(browse, input.source)?;
    Ok(Mix {
        id,
        name: input.name,
        source,
        ingredients: input
            .ingredients
            .into_iter()
            .map(Ingredient::from)
            .collect(),
    })
}

fn path(file: &MixesFile) -> MixResult<PathBuf> {
    file.0.clone().ok_or_else(MixErrorDto::no_file)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> MixResult<T> + Send + 'static,
) -> MixResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| MixErrorDto::new(MixErrorKind::Failed, "The mixes file could not be read"))?
}

/// The saved mixes of `source` (Navidrome: the current server's), in file
/// order.
pub async fn list(
    file: &MixesFile,
    browse: &BrowseHandle,
    source: SourceKind,
) -> MixResult<Vec<MixDto>> {
    let source = mix_source(browse, source)?;
    let Some(path) = file.0.clone() else {
        // No data folder: nothing can have been saved.
        return Ok(Vec::new());
    };
    blocking(move || {
        let (mixes, outcome) = store::load(&path);
        match outcome {
            LoadOutcome::Newer { .. } => Err(StoreError::Newer.into()),
            LoadOutcome::Rejected { reason, .. } => {
                tracing::warn!(%reason, "mixes file unusable; set aside");
                Ok(Vec::new())
            }
            LoadOutcome::Missing | LoadOutcome::Loaded { .. } => Ok(mixes
                .iter()
                .filter(|m| m.source == source)
                .map(MixDto::of)
                .collect()),
        }
    })
    .await
}

/// Saves a new mix; returns it as saved (cleaned).
pub async fn add(file: &MixesFile, browse: &BrowseHandle, input: MixInputDto) -> MixResult<MixDto> {
    let path = path(file)?;
    let mix = mix_from_input(browse, MixId::random(), input)?;
    blocking(move || {
        let id = mix.id.clone();
        let saved = store::add(&path, mix)?;
        saved
            .iter()
            .find(|m| m.id == id)
            .map(MixDto::of)
            .ok_or_else(|| MixErrorDto::new(MixErrorKind::Failed, "The mix was not saved"))
    })
    .await
}

fn parse_id(id: &str) -> MixResult<MixId> {
    MixId::parse(id).ok_or_else(|| MixErrorDto::from(StoreError::NotFound))
}

/// Replaces mix `id` with the recipe; returns it as saved. The saved mix
/// must be of the recipe's source now (Navidrome: the current server and
/// account), else `otherSource` and nothing changes.
pub async fn update(
    file: &MixesFile,
    browse: &BrowseHandle,
    id: String,
    input: MixInputDto,
) -> MixResult<MixDto> {
    let path = path(file)?;
    let mix = mix_from_input(browse, parse_id(&id)?, input)?;
    blocking(move || {
        let id = mix.id.clone();
        let saved = store::update(&path, mix)?;
        saved
            .iter()
            .find(|m| m.id == id)
            .map(MixDto::of)
            .ok_or_else(|| StoreError::NotFound.into())
    })
    .await
}

/// Deletes mix `id` of `source` (Navidrome: the current server's). One
/// already gone is no error; one of another source (the server or account
/// changed since it was listed) is refused with `otherSource`.
pub async fn delete(
    file: &MixesFile,
    browse: &BrowseHandle,
    id: String,
    source: SourceKind,
) -> MixResult<()> {
    let path = path(file)?;
    let id = parse_id(&id)?;
    let source = mix_source(browse, source)?;
    blocking(move || {
        store::delete(&path, &id, &source)
            .map(drop)
            .map_err(Into::into)
    })
    .await
}

#[tauri::command]
pub async fn list_mixes(
    file: State<'_, MixesFile>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
) -> MixResult<Vec<MixDto>> {
    list(&file, &browse, source).await
}

#[tauri::command]
pub async fn add_mix(
    file: State<'_, MixesFile>,
    browse: State<'_, BrowseHandle>,
    mix: MixInputDto,
) -> MixResult<MixDto> {
    add(&file, &browse, mix).await
}

#[tauri::command]
pub async fn update_mix(
    file: State<'_, MixesFile>,
    browse: State<'_, BrowseHandle>,
    id: String,
    mix: MixInputDto,
) -> MixResult<MixDto> {
    update(&file, &browse, id, mix).await
}

#[tauri::command]
pub async fn delete_mix(
    file: State<'_, MixesFile>,
    browse: State<'_, BrowseHandle>,
    id: String,
    source: SourceKind,
) -> MixResult<()> {
    delete(&file, &browse, id, source).await
}

// ----- the editor's search -------------------------------------------------------

type SearchResult = Result<Vec<IngredientDto>, BrowseError>;

fn contains(name: &str, needle: &str) -> bool {
    needle.is_empty() || name.to_lowercase().contains(needle)
}

/// Genres of `source` whose name contains `query` (every genre for an
/// empty query), at most [`MAX_GENRE_RESULTS`], A–Z.
pub async fn search_genres(
    browse: &BrowseHandle,
    source: SourceKind,
    query: String,
) -> SearchResult {
    let needle = crate::browse::cap_query(&query).to_lowercase();
    match source {
        SourceKind::Local => {
            browse
                .local(move |view, _| {
                    let tags = view.library.tags();
                    Ok(tags
                        .genres()
                        .iter()
                        .map(|&g| tags.genre(g).name())
                        .filter(|name| contains(name, &needle))
                        .take(MAX_GENRE_RESULTS)
                        .map(|name| IngredientDto::Genre {
                            name: name.to_owned(),
                        })
                        .collect())
                })
                .await
        }
        SourceKind::Navidrome => {
            let genres = browse.fetch(|c| async move { c.genres().await }).await?;
            Ok(genres
                .iter()
                .filter(|g| crate::browse::check_genre(g.query()).is_ok())
                .filter(|g| contains(&g.name, &needle))
                .take(MAX_GENRE_RESULTS)
                .map(|g| IngredientDto::Genre {
                    name: g.query().to_owned(),
                })
                .collect())
        }
    }
}

/// Artists of `source` whose name contains `query` (none for an empty
/// query). Navidrome: `search3`'s artists, with their ids.
pub async fn search_artists(
    browse: &BrowseHandle,
    source: SourceKind,
    query: String,
) -> SearchResult {
    let query = crate::browse::cap_query(&query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    match source {
        SourceKind::Local => {
            browse
                .local(move |view, _| {
                    let tags = view.library.tags();
                    let (artists, _) = tags.search(&query, MAX_NAME_RESULTS);
                    Ok(artists
                        .into_iter()
                        .map(|a| IngredientDto::Artist {
                            name: tags.artist(a).name().to_owned(),
                            id: None,
                        })
                        .collect())
                })
                .await
        }
        SourceKind::Navidrome => {
            let found = browse
                .fetch(|c| async move { c.search(&query).await })
                .await?;
            Ok(found
                .artists
                .into_iter()
                .map(|a| IngredientDto::Artist {
                    name: a.name,
                    id: Some(a.id),
                })
                .collect())
        }
    }
}

/// Albums of `source` whose name contains `query` (none for an empty
/// query), each with its (album) artist. Navidrome: `search3`'s albums,
/// with their ids; an album without an artist is left out (the resolver
/// could not find it by name).
pub async fn search_albums(
    browse: &BrowseHandle,
    source: SourceKind,
    query: String,
) -> SearchResult {
    let query = crate::browse::cap_query(&query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    match source {
        SourceKind::Local => {
            browse
                .local(move |view, _| {
                    let tags = view.library.tags();
                    let (_, albums) = tags.search(&query, MAX_NAME_RESULTS);
                    Ok(albums
                        .into_iter()
                        .map(|a| {
                            let album = tags.album(a);
                            IngredientDto::Album {
                                artist: tags.artist(album.artist()).name().to_owned(),
                                name: album.name().to_owned(),
                                id: None,
                            }
                        })
                        .collect())
                })
                .await
        }
        SourceKind::Navidrome => {
            let found = browse
                .fetch(|c| async move { c.search(&query).await })
                .await?;
            Ok(found
                .albums
                .into_iter()
                .filter_map(|a| {
                    Some(IngredientDto::Album {
                        artist: a.artist?,
                        name: a.name,
                        id: Some(a.id),
                    })
                })
                .collect())
        }
    }
}

#[tauri::command]
pub async fn mix_search_genres(
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    query: String,
) -> SearchResult {
    search_genres(&browse, source, query).await
}

#[tauri::command]
pub async fn mix_search_artists(
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    query: String,
) -> SearchResult {
    search_artists(&browse, source, query).await
}

#[tauri::command]
pub async fn mix_search_albums(
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    query: String,
) -> SearchResult {
    search_albums(&browse, source, query).await
}

#[cfg(test)]
mod tests;
