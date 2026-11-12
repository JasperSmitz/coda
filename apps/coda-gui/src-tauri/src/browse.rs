//! Browsing the local library and the Navidrome server for the UI.
//!
//! Browse commands do not go through the engine actor's channel and never
//! touch `Player`: they read through a [`BrowseHandle`], a cheap shared
//! read-only view the engine keeps current.
//!
//! - **Local:** the engine owns the [`Library`] inside the handle and is
//!   its only writer. It never waits for the lock: it writes with
//!   [`BrowseHandle::try_write_local`] and keeps a change for later while a
//!   browse read holds the lock (see `engine/library.rs`). Commands read
//!   it on a blocking thread, so a long read (a search, a 5,000-album
//!   list) never runs on the engine task, and the engine's own reads never
//!   wait either (no writer ever waits). Every replacement of the library
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
    /// Album fetches of one artist play-all (within `fetches`).
    play_fetches: Semaphore,
    covers: Arc<CoverService>,
}

/// The shared browse state; see the module docs. Cheap to clone.
#[derive(Clone)]
pub struct BrowseHandle(Arc<Shared>);

/// Write access to the local library (see [`BrowseHandle::try_write_local`]).
pub struct LocalWriter<'a>(std::sync::RwLockWriteGuard<'a, LocalState>);

impl LocalWriter<'_> {
    /// Replaces the library (a new music folder or a rescan) and bumps the
    /// generation, so ids handed out before are stale.
    pub fn replace(&mut self, library: Option<Library>) {
        self.0.generation += 1;
        self.0.library = library;
    }

    /// The library to change in place (scan results). Ids stay valid: the
    /// library only grows.
    pub fn library_mut(&mut self) -> Option<&mut Library> {
        self.0.library.as_mut()
    }
}

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
            play_fetches: Semaphore::new(coda_core::navidrome::browse::MAX_JOB_FETCHES),
            covers,
        }))
    }

    pub fn covers(&self) -> &CoverService {
        &self.0.covers
    }

    // ----- the engine's side --------------------------------------------

    /// Replaces the library and bumps the generation, waiting for the
    /// lock. For tests and setup; the engine uses [`Self::try_write_local`].
    pub fn replace_library(&self, library: Option<Library>) {
        let local = self.0.local.write().unwrap_or_else(|e| e.into_inner());
        LocalWriter(local).replace(library);
    }

    /// The library for writing, unless a reader holds it right now (then
    /// `None` at once: the engine never waits on browse work).
    pub fn try_write_local(&self) -> Option<LocalWriter<'_>> {
        match self.0.local.try_write() {
            Ok(guard) => Some(LocalWriter(guard)),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(LocalWriter(e.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
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
    pub(crate) fn client(&self) -> Result<(Client, u64), BrowseError> {
        let remote = self.0.remote.read().unwrap_or_else(|e| e.into_inner());
        match &remote.client {
            Some(client) => Ok((client.clone(), remote.generation)),
            None => Err(BrowseError::no_server()),
        }
    }

    pub fn client_generation(&self) -> u64 {
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
        self.fetch_raw(f).await?.map_err(|error| {
            tracing::warn!(detail = %error.detail(), "Navidrome browse request failed");
            BrowseError::new(BrowseErrorKind::Failed, error.to_string())
        })
    }

    /// Like [`Self::fetch`], but hands the request's own result back
    /// (unlogged): the outer error is only "no server", "shutting down" or
    /// "stale".
    pub async fn fetch_raw<T, E, F, Fut>(&self, f: F) -> Result<Result<T, E>, BrowseError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = Result<T, E>>,
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
        Ok(result)
    }
}

// ----- ids and input bounds -------------------------------------------------

/// Longest id or search query taken from the UI (ids: as the server's
/// are bounded; queries: a pasted wall of text is not a search).
pub const MAX_INPUT_CHARS: usize = 256;

/// A Navidrome id from the UI: non-empty and bounded.
pub fn check_id(id: &str) -> Result<(), BrowseError> {
    if id.is_empty() || id.chars().count() > MAX_INPUT_CHARS {
        return Err(BrowseError::not_found("Item"));
    }
    Ok(())
}

/// A search query, trimmed and cut at [`MAX_INPUT_CHARS`].
pub fn cap_query(query: &str) -> String {
    query.trim().chars().take(MAX_INPUT_CHARS).collect()
}

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
