//! Navidrome lyrics search: the background sync of the server's lyrics
//! cache (`coda_core::lyrics::cache`) and the loaded cache that
//! `navidrome_lyrics_search` reads.
//!
//! - **When:** about [`LAUNCH_DELAY`] after launch when a server is
//!   configured, at once when a server is added or changed, when switched
//!   on, and on Rebuild. Never on a timer while the app is open.
//! - **One at a time,** as a task off the engine actor (the cache is
//!   loaded on a blocking thread); a new start aborts the running one.
//!   A server change starts over for the new server (each server has its
//!   own file; the old one is kept). Disconnecting deletes the server's
//!   file; switching the index off stops the sync, deletes the file and
//!   drops the cache from memory.
//! - **Quit:** [`LyricsIndex::shutdown`] aborts the sync and saves what it
//!   has (bounded), so the next launch goes on from there.
//! - **Status:** a [`LyricsIndexDto`] the Settings block shows, sent as
//!   the `lyrics-index` event when it changes (see `lib.rs`).
//!
//! The switch is the GUI preference `lyrics_index` (`settings.toml`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use coda_core::lyrics::cache::{self, Pause, State, Status, Store, SyncOptions};
use coda_core::navidrome::Client;
use serde::Serialize;
use tauri::State as TauriState;
use tokio::sync::watch;

use crate::browse::BrowseHandle;
use crate::theme::Appearance;

/// How long after launch the first sync starts (startup traffic first).
pub const LAUNCH_DELAY: Duration = Duration::from_secs(30);
/// Upper bound for the save at quit.
pub const SHUTDOWN_SAVE_BOUND: Duration = Duration::from_secs(3);

/// What the index is doing, for Settings and the search page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexStateDto {
    /// Switched off.
    Off,
    /// No Navidrome server.
    NoServer,
    /// Reading the cache file.
    Loading,
    /// Loaded; no sync has run yet.
    Idle,
    /// Listing albums and fetching new ones (`done` of `todo` albums).
    Albums,
    /// Fetching lyrics (`done` of `todo` songs).
    Songs,
    UpToDate,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PauseDto {
    Unreachable,
    Rejected,
    Failed,
}

/// The index's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsIndexDto {
    pub state: IndexStateDto,
    /// Why it is paused.
    pub pause: Option<PauseDto>,
    pub done: usize,
    pub todo: usize,
    /// Songs known, songs checked at least once, songs with lyrics.
    pub songs: usize,
    pub checked: usize,
    pub with_lyrics: usize,
}

impl LyricsIndexDto {
    fn bare(state: IndexStateDto) -> Self {
        Self {
            state,
            pause: None,
            done: 0,
            todo: 0,
            songs: 0,
            checked: 0,
            with_lyrics: 0,
        }
    }

    fn from_status(status: Status) -> Self {
        let (state, done, todo, pause) = match status.state {
            State::Idle => (IndexStateDto::Idle, 0, 0, None),
            State::Albums { done, todo } => (IndexStateDto::Albums, done, todo, None),
            State::Songs { done, todo } => (IndexStateDto::Songs, done, todo, None),
            State::UpToDate => (IndexStateDto::UpToDate, 0, 0, None),
            State::Paused(pause) => (
                IndexStateDto::Paused,
                0,
                0,
                Some(match pause {
                    Pause::Unreachable => PauseDto::Unreachable,
                    Pause::Rejected => PauseDto::Rejected,
                    Pause::Failed => PauseDto::Failed,
                }),
            ),
        };
        Self {
            state,
            pause,
            done,
            todo,
            songs: status.songs,
            checked: status.checked,
            with_lyrics: status.with_lyrics,
        }
    }
}

/// How a sync is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// At launch: after [`LAUNCH_DELAY`].
    Launch,
    /// A server added or changed (`None`: disconnected): now.
    Changed,
}

struct Inner {
    /// The cache folder; `None` (tests, no `HOME`): never syncs.
    dir: Option<PathBuf>,
    state: Mutex<Running>,
    status: watch::Sender<LyricsIndexDto>,
}

#[derive(Default)]
struct Running {
    enabled: bool,
    client: Option<Client>,
    /// The current server's cache key.
    key: Option<String>,
    /// Its cache, once loaded.
    store: Option<Arc<Store>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Bumped by every start: a task of an older one installs nothing.
    generation: u64,
}

/// The lyrics index service. Cheap to clone.
#[derive(Clone)]
pub struct LyricsIndex(Arc<Inner>);

impl LyricsIndex {
    /// A service keeping caches in `dir` (`None`: none, and no sync).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self(Arc::new(Inner {
            dir,
            state: Mutex::new(Running {
                enabled: true,
                ..Running::default()
            }),
            status: watch::Sender::new(LyricsIndexDto::bare(IndexStateDto::NoServer)),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Running> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The switch as saved, before any server is set.
    pub fn set_enabled_at_start(&self, enabled: bool) {
        self.lock().enabled = enabled;
        self.publish();
    }

    /// The current status.
    pub fn status(&self) -> LyricsIndexDto {
        *self.0.status.borrow()
    }

    /// Status changes.
    pub fn subscribe(&self) -> watch::Receiver<LyricsIndexDto> {
        self.0.status.subscribe()
    }

    /// The loaded cache, when the index is on.
    pub fn store(&self) -> Option<Arc<Store>> {
        let running = self.lock();
        running.enabled.then(|| running.store.clone()).flatten()
    }

    /// Sends the status now.
    fn publish(&self) {
        let dto = {
            let running = self.lock();
            if !running.enabled {
                LyricsIndexDto::bare(IndexStateDto::Off)
            } else if running.client.is_none() || self.0.dir.is_none() {
                LyricsIndexDto::bare(IndexStateDto::NoServer)
            } else {
                match &running.store {
                    Some(store) => LyricsIndexDto::from_status(store.status()),
                    None => LyricsIndexDto::bare(IndexStateDto::Loading),
                }
            }
        };
        self.0.status.send_if_modified(|current| {
            let changed = *current != dto;
            *current = dto;
            changed
        });
    }

    /// The server changed (or is set at launch). Disconnecting deletes
    /// the old server's cache file.
    pub fn set_server(&self, client: Option<Client>, start: Start) {
        let key = client
            .as_ref()
            .map(|c| cache::server_key(c.base_url().as_str(), c.username()));
        {
            let mut running = self.lock();
            stop_task(&mut running);
            if client.is_none() && start == Start::Changed {
                self.delete_current(&mut running);
            }
            if key != running.key {
                running.store = None;
            }
            running.key = key;
            running.client = client;
        }
        let delay = match start {
            Start::Launch => LAUNCH_DELAY,
            Start::Changed => Duration::ZERO,
        };
        self.restart(delay, false);
    }

    /// Checks everything again now. `Err` when there is nothing to
    /// rebuild (off, or no server).
    pub fn rebuild(&self) -> Result<(), String> {
        {
            let running = self.lock();
            if !running.enabled {
                return Err("Lyrics search is off".into());
            }
            if running.client.is_none() || self.0.dir.is_none() {
                return Err("No Navidrome server is connected".into());
            }
        }
        self.restart(Duration::ZERO, true);
        Ok(())
    }

    /// Switches the index on (a sync starts now) or off (the sync stops,
    /// this server's file is deleted and the cache dropped).
    pub fn set_enabled(&self, enabled: bool) {
        {
            let mut running = self.lock();
            if running.enabled == enabled {
                return;
            }
            running.enabled = enabled;
            if !enabled {
                stop_task(&mut running);
                self.delete_current(&mut running);
            }
        }
        if enabled {
            self.restart(Duration::ZERO, false);
        } else {
            self.publish();
        }
    }

    /// Deletes the current server's file and drops its cache. The delete
    /// runs on a blocking thread; a save in progress finishes first and
    /// later ones write nothing.
    fn delete_current(&self, running: &mut Running) {
        let store = running.store.take();
        let (Some(dir), Some(key)) = (self.0.dir.clone(), running.key.clone()) else {
            return;
        };
        tauri::async_runtime::spawn_blocking(move || {
            let result = match store {
                Some(store) => store.delete_file(),
                None => cache::remove_file(&dir, &key),
            };
            match result {
                Ok(()) => tracing::info!("lyrics cache deleted"),
                Err(error) => tracing::warn!(%error, "could not delete the lyrics cache"),
            }
        });
    }

    /// Starts a sync of the current server after `delay` (aborting the
    /// one running), loading its cache first if needed.
    fn restart(&self, delay: Duration, rebuild: bool) {
        let mut running = self.lock();
        stop_task(&mut running);
        let generation = running.generation;
        let (Some(dir), Some(client), Some(key), true) = (
            self.0.dir.clone(),
            running.client.clone(),
            running.key.clone(),
            running.enabled,
        ) else {
            drop(running);
            self.publish();
            return;
        };
        let loaded = running.store.clone();
        let service = self.clone();
        running.task = Some(tauri::async_runtime::spawn(async move {
            let store = match loaded {
                Some(store) => store,
                None => {
                    let load_key = key.clone();
                    let opened =
                        tokio::task::spawn_blocking(move || Store::open(&dir, &load_key)).await;
                    let Ok(store) = opened else { return };
                    let store = Arc::new(store);
                    if !service.install_loaded(generation, &key, store.clone()) {
                        return;
                    }
                    store
                }
            };
            service.publish();
            tokio::time::sleep(delay).await;
            let mut changes = store.subscribe();
            let sync = cache::sync(&store, &client, SyncOptions::new(rebuild));
            tokio::pin!(sync);
            loop {
                tokio::select! {
                    _ = &mut sync => break,
                    changed = changes.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        service.publish();
                    }
                }
            }
            service.publish();
        }));
        drop(running);
        self.publish();
    }

    /// Installs a cache the start of `generation` loaded for `key`, unless
    /// anything changed since (another start, a stop, another server, the
    /// switch turned off): `false` then, and it is dropped. The task may
    /// be past its abort point when that happens, so this check, under
    /// the same lock as every change, is what keeps a late load out.
    fn install_loaded(&self, generation: u64, key: &str, store: Arc<Store>) -> bool {
        let mut running = self.lock();
        if running.generation != generation
            || !running.enabled
            || running.key.as_deref() != Some(key)
        {
            return false;
        }
        running.store = Some(store);
        true
    }

    /// Deletes the cache file of the server at `url` seen by `username`
    /// (a disconnect from Settings; the server's client may never have
    /// been set up, e.g. without its password at launch). The current
    /// server's file goes through the client swap instead.
    pub fn forget(&self, url: &str, username: &str) {
        let Ok(url) = coda_core::navidrome::client::parse_base_url(url) else {
            return;
        };
        let key = cache::server_key(url.as_str(), username);
        if self.lock().key.as_deref() == Some(key.as_str()) {
            return;
        }
        let Some(dir) = self.0.dir.clone() else {
            return;
        };
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = cache::remove_file(&dir, &key) {
                tracing::warn!(%error, "could not delete the lyrics cache");
            }
        });
    }

    /// Uses `store` as the current server's cache (tests).
    #[cfg(test)]
    pub(crate) fn install(&self, client: Client, store: Arc<Store>) {
        let mut running = self.lock();
        running.client = Some(client);
        running.store = Some(store);
    }

    /// At quit: stops the sync and saves what it has, within
    /// [`SHUTDOWN_SAVE_BOUND`].
    pub async fn shutdown(&self) {
        let store = {
            let mut running = self.lock();
            stop_task(&mut running);
            running.enabled.then(|| running.store.clone()).flatten()
        };
        if let Some(store) = store {
            let save = tokio::task::spawn_blocking(move || store.save_now());
            match tokio::time::timeout(SHUTDOWN_SAVE_BOUND, save).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => tracing::warn!(%error, "could not save the lyrics cache"),
                _ => tracing::warn!("the lyrics cache save at quit did not finish"),
            }
        }
    }
}

/// Aborts the running task and starts a new generation, so nothing the
/// old task loads is installed (see [`LyricsIndex::install_loaded`]).
fn stop_task(running: &mut Running) {
    if let Some(task) = running.task.take() {
        task.abort();
    }
    running.generation += 1;
}

// ----- commands -------------------------------------------------------------

/// The index's status, for Settings (later changes come as the
/// `lyrics-index` event).
#[tauri::command]
pub async fn lyrics_index_status(
    browse: TauriState<'_, BrowseHandle>,
) -> Result<LyricsIndexDto, String> {
    Ok(browse.lyrics().status())
}

/// Checks every album and song on the server again.
#[tauri::command]
pub async fn lyrics_index_rebuild(browse: TauriState<'_, BrowseHandle>) -> Result<(), String> {
    browse.lyrics().rebuild()
}

/// Saves the "Index lyrics for search" switch, then applies it; returns
/// the new status (so the switch never shows the old value meanwhile).
#[tauri::command]
pub async fn lyrics_index_set_enabled(
    appearance: TauriState<'_, Appearance>,
    browse: TauriState<'_, BrowseHandle>,
    enabled: bool,
) -> Result<LyricsIndexDto, String> {
    crate::theme::commands::save_prefs(&appearance, |prefs| prefs.lyrics_index = enabled).await?;
    browse.lyrics().set_enabled(enabled);
    Ok(browse.lyrics().status())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(user: &str) -> Client {
        Client::new(
            coda_core::navidrome::client::parse_base_url("http://127.0.0.1:9").unwrap(),
            user.into(),
            coda_core::navidrome::Password::new("pw".into()),
        )
        .unwrap()
    }

    #[test]
    fn without_a_folder_nothing_runs_and_the_status_says_why() {
        let index = LyricsIndex::new(None);
        assert_eq!(index.status().state, IndexStateDto::NoServer);
        index.set_server(Some(client("a")), Start::Launch);
        assert_eq!(index.status().state, IndexStateDto::NoServer);
        assert!(index.store().is_none());
        assert!(index.rebuild().is_err());
        index.set_enabled(false);
        assert_eq!(index.status().state, IndexStateDto::Off);
        assert_eq!(index.rebuild(), Err("Lyrics search is off".into()));
    }

    #[test]
    fn statuses_map_to_the_dto() {
        let status = Status {
            state: State::Songs { done: 3, todo: 9 },
            songs: 20,
            checked: 11,
            with_lyrics: 7,
        };
        let dto = LyricsIndexDto::from_status(status);
        assert_eq!(dto.state, IndexStateDto::Songs);
        assert_eq!(
            (dto.done, dto.todo, dto.songs, dto.checked, dto.with_lyrics),
            (3, 9, 20, 11, 7)
        );
        let paused = LyricsIndexDto::from_status(Status {
            state: State::Paused(Pause::Rejected),
            ..status
        });
        assert_eq!(paused.pause, Some(PauseDto::Rejected));
        let json = serde_json::to_value(dto).unwrap();
        assert_eq!(json["state"], "songs");
        assert_eq!(json["withLyrics"], 7);
    }

    #[tokio::test]
    async fn switching_off_deletes_the_file_and_a_server_change_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let index = LyricsIndex::new(Some(dir.path().to_owned()));
        let a = client("a");
        let key_a = cache::server_key(a.base_url().as_str(), "a");
        std::fs::write(cache::file_path(dir.path(), &key_a), b"{}").unwrap();
        // A launch loads the cache (the sync waits LAUNCH_DELAY).
        index.set_server(Some(a.clone()), Start::Launch);
        for _ in 0..100 {
            if index.store().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(index.store().is_some());
        assert_eq!(index.status().state, IndexStateDto::Idle);
        // Another server: the old file stays.
        index.set_server(Some(client("b")), Start::Launch);
        assert!(cache::file_path(dir.path(), &key_a).exists());
        // Back to the first, then off: its file goes.
        index.set_server(Some(a), Start::Launch);
        index.set_enabled(false);
        assert_eq!(index.status().state, IndexStateDto::Off);
        assert!(index.store().is_none());
        for _ in 0..100 {
            if !cache::file_path(dir.path(), &key_a).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!cache::file_path(dir.path(), &key_a).exists());
        index.shutdown().await;
    }

    #[tokio::test]
    async fn the_search_command_reads_the_loaded_cache() {
        let dir = tempfile::tempdir().unwrap();
        let file = serde_json::json!({
            "v": 1, "refreshed": 1,
            "albums": [{ "id": "al", "n": 2, "t": 1 }],
            "songs": [
                { "id": "s1", "al": "al", "ti": "River", "ar": "Band", "alb": "First",
                  "n": 1, "t": 1, "ly": "Golden river\nruns to the sea" },
                { "id": "s2", "al": "al", "ti": "River Song", "ar": "Band", "alb": "First",
                  "n": 2, "t": 1, "ly": "a golden river song" }
            ]
        });
        std::fs::write(cache::file_path(dir.path(), "k"), file.to_string()).unwrap();
        let store = Arc::new(Store::open(dir.path(), "k"));
        let index = LyricsIndex::new(None);
        index.install(client("a"), store);
        let browse = BrowseHandle::with_lyrics(
            Arc::new(crate::cover::CoverService::new(None)),
            index.clone(),
        );
        // s2 is in the main results: left out.
        let found = browse
            .navidrome_lyrics_search("golden river".into(), vec!["s2".into()])
            .await
            .unwrap();
        assert_eq!(found.total, 1);
        assert_eq!(found.indexing, None);
        let m = &found.matches[0];
        assert_eq!(m.track.title, "River");
        assert!(matches!(
            &m.track.source,
            crate::dto::SourceDto::Navidrome { song_id, .. } if song_id == "s1"
        ));
        assert_eq!(m.track.album_id.as_deref(), Some("al"));
        let parts: Vec<(&str, bool)> = m
            .snippet
            .iter()
            .map(|p| (p.text.as_str(), p.matched))
            .collect();
        assert_eq!(parts, [("Golden river", true)]);
        // Off: no section.
        index.set_enabled(false);
        let found = browse
            .navidrome_lyrics_search("golden river".into(), Vec::new())
            .await
            .unwrap();
        assert_eq!((found.total, found.matches.len()), (0, 0));
    }

    #[tokio::test]
    async fn a_late_load_is_never_installed() {
        let dir = tempfile::tempdir().unwrap();
        let index = LyricsIndex::new(Some(dir.path().to_owned()));
        let (a, b) = (client("a"), client("b"));
        let key_a = cache::server_key(a.base_url().as_str(), "a");
        let key_b = cache::server_key(b.base_url().as_str(), "b");
        index.set_server(Some(a.clone()), Start::Launch);
        let started = index.lock().generation;
        // Server B arrives while A's load is still running.
        index.set_server(Some(b), Start::Changed);
        assert!(!index.install_loaded(started, &key_a, Arc::new(Store::in_memory())));
        // Even with B's key, an older generation is refused.
        assert!(!index.install_loaded(started, &key_b, Arc::new(Store::in_memory())));
        // The current generation and key are accepted, unless switched off.
        let current = index.lock().generation;
        index.set_enabled(false);
        let after_off = index.lock().generation;
        assert_ne!(current, after_off, "switching off starts a generation");
        assert!(!index.install_loaded(after_off, &key_b, Arc::new(Store::in_memory())));
        assert!(index.store().is_none());
        index.shutdown().await;
    }

    #[tokio::test]
    async fn forgetting_a_server_without_a_client_deletes_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let index = LyricsIndex::new(Some(dir.path().to_owned()));
        // At launch the client could not be set up (no password).
        index.set_server(None, Start::Launch);
        let url = coda_core::navidrome::client::parse_base_url("http://127.0.0.1:9").unwrap();
        let key = cache::server_key(url.as_str(), "a");
        std::fs::write(cache::file_path(dir.path(), &key), b"{}").unwrap();
        index.forget("http://127.0.0.1:9", "a");
        for _ in 0..100 {
            if !cache::file_path(dir.path(), &key).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!cache::file_path(dir.path(), &key).exists());
    }

    #[tokio::test]
    async fn disconnecting_deletes_the_servers_file() {
        let dir = tempfile::tempdir().unwrap();
        let index = LyricsIndex::new(Some(dir.path().to_owned()));
        let a = client("a");
        let key_a = cache::server_key(a.base_url().as_str(), "a");
        std::fs::write(cache::file_path(dir.path(), &key_a), b"{}").unwrap();
        index.set_server(Some(a), Start::Launch);
        index.set_server(None, Start::Changed);
        for _ in 0..100 {
            if !cache::file_path(dir.path(), &key_a).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!cache::file_path(dir.path(), &key_a).exists());
        assert_eq!(index.status().state, IndexStateDto::NoServer);
    }
}
