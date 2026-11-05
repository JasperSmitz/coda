//! Browsing the local library and the Navidrome server for the UI.
//!
//! Browse commands do not go through the engine actor's channel and never
//! touch `Player`: they read through a [`BrowseHandle`], a cheap shared
//! read-only view the engine keeps current.
//!
//! - **Local:** the engine owns the [`Library`] inside the handle and is
//!   its only writer (scan batches, a short write lock each). Commands
//!   read it on a blocking thread, so a long read (a search, a 5,000-album
//!   list) never runs on the engine task. Every replacement of the library
//!   bumps its *generation*; local ids carry it (`"<generation>:<n>"`), so
//!   an id from an older library is rejected as stale instead of naming
//!   another item.
//! - **Navidrome:** the engine publishes its current [`Client`] (cheap to
//!   clone) with a generation; a result fetched with an older client is
//!   dropped. At most [`MAX_CONCURRENT_FETCHES`] requests run at once, as
//!   in Coda's browser.
//!
//! Track DTOs for the queue and for browse lists are built here
//! ([`track_dto`]), so both name local tracks the same way.

pub mod commands;
pub mod dto;
pub mod local;
pub mod navidrome;

use std::sync::{Arc, RwLock};

use coda_core::cover::CoverKey;
use coda_core::library::Library;
use coda_core::model::{Track, TrackSource};
use coda_core::navidrome::Client;
pub use coda_core::navidrome::browse::MAX_CONCURRENT_FETCHES;
use tokio::sync::Semaphore;

use crate::cover::CoverService;
use crate::dto::{SourceDto, TrackDto};
use dto::{BrowseError, BrowseErrorKind};

#[derive(Default)]
struct LocalState {
    generation: u64,
    library: Option<Library>,
}

#[derive(Default)]
struct RemoteState {
    generation: u64,
    client: Option<Client>,
}

struct Shared {
    local: RwLock<LocalState>,
    remote: RwLock<RemoteState>,
    fetches: Semaphore,
    covers: Arc<CoverService>,
}

/// The shared browse state; see the module docs. Cheap to clone.
#[derive(Clone)]
pub struct BrowseHandle(Arc<Shared>);

/// A read view of the local library at one generation.
pub struct LocalView<'a> {
    pub library: &'a Library,
    pub generation: u64,
}

impl BrowseHandle {
    pub fn new(covers: Arc<CoverService>) -> Self {
        Self(Arc::new(Shared {
            local: RwLock::new(LocalState::default()),
            remote: RwLock::new(RemoteState::default()),
            fetches: Semaphore::new(MAX_CONCURRENT_FETCHES),
            covers,
        }))
    }

    pub fn covers(&self) -> &CoverService {
        &self.0.covers
    }

    // ----- the engine's side --------------------------------------------

    /// Replaces the library (a new music folder or a rescan) and bumps the
    /// generation, so ids handed out before are stale.
    pub fn replace_library(&self, library: Option<Library>) {
        let mut local = self.0.local.write().unwrap_or_else(|e| e.into_inner());
        local.generation += 1;
        local.library = library;
    }

    /// Changes the library in place (scan results). Ids stay valid: the
    /// library only grows. `None` without a library.
    pub fn update_library<R>(&self, f: impl FnOnce(&mut Library) -> R) -> Option<R> {
        let mut local = self.0.local.write().unwrap_or_else(|e| e.into_inner());
        local.library.as_mut().map(f)
    }

    /// Reads the library (`None` without one). Keep it short on the engine
    /// task; browse commands call it on a blocking thread.
    pub fn read_library<R>(&self, f: impl FnOnce(Option<LocalView<'_>>) -> R) -> R {
        let local = self.0.local.read().unwrap_or_else(|e| e.into_inner());
        f(local.library.as_ref().map(|library| LocalView {
            library,
            generation: local.generation,
        }))
    }

    pub fn library_generation(&self) -> u64 {
        self.0
            .local
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }

    /// Publishes the current Navidrome client (or none) under a new
    /// generation.
    pub fn set_client(&self, client: Option<Client>) {
        let mut remote = self.0.remote.write().unwrap_or_else(|e| e.into_inner());
        remote.generation += 1;
        remote.client = client;
    }

    // ----- the commands' side -------------------------------------------

    /// Runs `f` over the library on a blocking thread.
    pub async fn local<R, F>(&self, f: F) -> Result<R, BrowseError>
    where
        R: Send + 'static,
        F: FnOnce(LocalView<'_>, &CoverService) -> Result<R, BrowseError> + Send + 'static,
    {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || {
            handle.read_library(|view| match view {
                Some(view) => f(view, handle.covers()),
                None => Err(BrowseError::no_library()),
            })
        })
        .await
        .map_err(|_| BrowseError::new(BrowseErrorKind::Failed, "The library read failed"))?
    }

    /// The current client and its generation.
    fn client(&self) -> Result<(Client, u64), BrowseError> {
        let remote = self.0.remote.read().unwrap_or_else(|e| e.into_inner());
        match &remote.client {
            Some(client) => Ok((client.clone(), remote.generation)),
            None => Err(BrowseError::no_server()),
        }
    }

    fn client_generation(&self) -> u64 {
        self.0
            .remote
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }

    /// One server request with the current client, within the fetch limit.
    /// A result that arrives after the client changed is dropped as stale.
    pub async fn fetch<T, F, Fut>(&self, f: F) -> Result<T, BrowseError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = Result<T, coda_core::navidrome::Error>>,
    {
        let (client, generation) = self.client()?;
        let result = {
            let _permit = self
                .0
                .fetches
                .acquire()
                .await
                .map_err(|_| BrowseError::new(BrowseErrorKind::Failed, "Shutting down"))?;
            f(client).await
        };
        if self.client_generation() != generation {
            return Err(BrowseError::new(
                BrowseErrorKind::Stale,
                "The server connection changed; reload",
            ));
        }
        result.map_err(|error| {
            tracing::warn!(detail = %error.detail(), "Navidrome browse request failed");
            BrowseError::new(BrowseErrorKind::Failed, error.to_string())
        })
    }
}

// ----- ids ----------------------------------------------------------------

/// A local id: `"<generation>:<n>"`.
pub fn local_id(generation: u64, n: u32) -> String {
    format!("{generation}:{n}")
}

/// Parses a local id of `generation`; `Err(stale)` for another
/// generation, `Err(not found)` for anything malformed.
pub fn parse_local_id(id: &str, generation: u64) -> Result<u32, BrowseError> {
    let (g, n) = id
        .split_once(':')
        .and_then(|(g, n)| Some((g.parse::<u64>().ok()?, n.parse::<u32>().ok()?)))
        .ok_or_else(|| BrowseError::not_found("Item"))?;
    if g != generation {
        return Err(BrowseError::stale());
    }
    Ok(n)
}

// ----- tracks -------------------------------------------------------------

/// The DTO for `track`. Local tracks are named by their id in `local`
/// (and linked to their album and artist there); a local track not in the
/// library (or without one) has no id and cannot be played by ref.
pub fn track_dto(track: &Track, local: Option<&LocalView<'_>>, covers: &CoverService) -> TrackDto {
    let (source, album_id, artist_id) = match &track.source {
        TrackSource::Local { path } => {
            let found = local.and_then(|view| {
                let id = view.library.track_id_by_path(path)?;
                let tags = view.library.tags();
                Some((
                    local_id(view.generation, id.0),
                    tags.album_of(track).map(|a| local_id(view.generation, a.0)),
                    tags.artist_of(track)
                        .map(|a| local_id(view.generation, a.0)),
                ))
            });
            match found {
                Some((id, album, artist)) => (SourceDto::Local { id: Some(id) }, album, artist),
                None => (SourceDto::Local { id: None }, None, None),
            }
        }
        TrackSource::Navidrome { song_id } => (
            SourceDto::Navidrome {
                song_id: song_id.clone(),
                cover_art: track.cover_art.clone(),
            },
            track.album_id.clone(),
            track.artist_id.clone(),
        ),
    };
    TrackDto {
        source,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        album_artist: track.album_artist.clone(),
        track_no: track.track_no,
        disc_no: track.disc_no,
        duration_ms: track.duration.map(|d| d.as_millis() as u64),
        cover_url: covers.url_for(track),
        album_id,
        artist_id,
    }
}

/// The cover URL for a Navidrome `coverArt` id.
fn remote_cover(covers: &CoverService, cover_art: Option<&String>) -> Option<String> {
    covers.url_for_key(CoverKey::Navidrome(cover_art?.clone()))
}

#[cfg(test)]
mod tests;
