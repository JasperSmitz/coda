//! The engine's side of the play history ([`crate::history`]): context
//! plays recorded from the play intents, listens counted with Coda's
//! scrobble rule (half the track or 4 minutes, [`Listen`]) for every
//! source, debounced saves, and the Home commands' requests.

use std::collections::HashSet;

use super::*;
use crate::browse::dto::LocalContextDto;
use crate::dto::{NamedContextDto, PlayContextDto};
use crate::history::{
    self, Context, ContextKind, DailyAlbum, History, HistoryFile, HomeRows, Scope, SourceKind,
};

/// The listen counted for the history (any source).
#[derive(Debug, Clone)]
pub(crate) struct Heard {
    listen: Listen,
    contexts: Vec<Context>,
}

/// The context the queue was last played from, and its entries.
#[derive(Debug, Clone)]
pub(crate) struct PlayedFrom {
    context: Context,
    entries: HashSet<QueueEntryId>,
}

/// Save state of the history (like the session's, by revision).
#[derive(Debug, Default)]
pub(crate) struct HistorySave {
    saved: u64,
    due: Option<Instant>,
    failures: u32,
}

pub enum HistoryMsg {
    /// The Home rows of a source (unresolved).
    Home(SourceKind, oneshot::Sender<HomeRows>),
    /// The cached Navidrome Daily Picks of a date.
    Daily(String, oneshot::Sender<Option<Vec<DailyAlbum>>>),
    /// Caches a date's picks; answers the day's picks (the first stay).
    SetDaily(String, Vec<DailyAlbum>, oneshot::Sender<Vec<DailyAlbum>>),
}

impl EngineHandle {
    async fn history_request<T>(
        &self,
        msg: impl FnOnce(oneshot::Sender<T>) -> HistoryMsg,
    ) -> Result<T, EngineGone> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Msg::History(msg(tx)))
            .await
            .map_err(|_| EngineGone)?;
        rx.await.map_err(|_| EngineGone)
    }

    pub async fn home_rows(&self, source: SourceKind) -> Result<HomeRows, EngineGone> {
        self.history_request(|tx| HistoryMsg::Home(source, tx))
            .await
    }

    pub async fn daily_picks(&self, date: String) -> Result<Option<Vec<DailyAlbum>>, EngineGone> {
        self.history_request(|tx| HistoryMsg::Daily(date, tx)).await
    }

    pub async fn set_daily_picks(
        &self,
        date: String,
        albums: Vec<DailyAlbum>,
    ) -> Result<Vec<DailyAlbum>, EngineGone> {
        self.history_request(|tx| HistoryMsg::SetDaily(date, albums, tx))
            .await
    }
}

/// Longest wait before retrying a failed history write.
const HISTORY_RETRY_MAX: Duration = Duration::from_secs(60);

impl Engine {
    pub(super) fn handle_history_msg(&mut self, msg: HistoryMsg, now: Instant) {
        match msg {
            HistoryMsg::Home(source, reply) => {
                let rows = self
                    .scope(source)
                    .map(|scope| self.history.home_rows(&scope))
                    .unwrap_or_default();
                let _ = reply.send(rows);
            }
            HistoryMsg::Daily(date, reply) => {
                let picks = self
                    .server
                    .as_deref()
                    .and_then(|server| self.history.daily(&date, server))
                    .map(<[_]>::to_vec);
                let _ = reply.send(picks);
            }
            HistoryMsg::SetDaily(date, albums, reply) => {
                // Fetched with the current client (a result of an older
                // one is dropped as stale before it gets here).
                let picks = match self.server.clone() {
                    Some(server) => self.history.set_daily(date, server, albums),
                    None => Vec::new(),
                };
                self.history_changed(now);
                let _ = reply.send(picks);
            }
        }
    }

    /// The part of the history `source` shows: the local library's, or the
    /// current server's (none without a server).
    pub fn scope(&self, source: SourceKind) -> Option<Scope> {
        match source {
            SourceKind::Local => Some(Scope::Local),
            SourceKind::Navidrome => self.server.clone().map(Scope::Navidrome),
        }
    }

    /// The history startup loaded (before [`Engine::start`]).
    pub fn set_history(&mut self, history: History) {
        self.history_save.saved = history.revision();
        self.history = history;
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub(super) fn history_changed(&mut self, now: Instant) {
        if self.history.revision() != self.history_save.saved && self.history_save.due.is_none() {
            self.history_save.due = Some(now + SAVE_DEBOUNCE);
        }
    }

    /// The history to write, once [`SAVE_DEBOUNCE`] passed since the
    /// first unsaved change; with its revision.
    pub fn take_due_history(&mut self, now: Instant) -> Option<(u64, HistoryFile)> {
        if self.history_save.due.is_none_or(|due| now < due) {
            return None;
        }
        self.history_save.due = None;
        (self.history.revision() != self.history_save.saved)
            .then(|| (self.history.revision(), self.history.to_file()))
    }

    /// The exit write, if anything changed since the last one.
    pub fn take_final_history(&mut self) -> Option<(u64, HistoryFile)> {
        self.history_save.due = None;
        (self.history.revision() != self.history_save.saved)
            .then(|| (self.history.revision(), self.history.to_file()))
    }

    /// A write of `revision` finished.
    pub fn history_saved(&mut self, revision: Option<u64>, now: Instant) {
        match revision {
            Some(revision) => {
                self.history_save.saved = self.history_save.saved.max(revision);
                self.history_save.failures = 0;
                // Changes made while it was written.
                self.history_changed(now);
            }
            None => {
                let delay = SAVE_DEBOUNCE
                    .saturating_mul(1 << self.history_save.failures.min(5))
                    .min(HISTORY_RETRY_MAX);
                self.history_save.failures = self.history_save.failures.saturating_add(1);
                self.history_save.due = Some(now + delay);
            }
        }
    }

    // ----- context plays ----------------------------------------------

    /// The history context a play intent's descriptor names, if it is
    /// valid (a local id of the current library; a Navidrome id).
    pub(super) fn play_context(&self, dto: PlayContextDto) -> Option<Context> {
        let (kind, named) = match dto {
            PlayContextDto::Folder { path } => {
                return self.browse.read_library(|view| {
                    view?.library.tree().dir(&path)?;
                    Some(history::local_folder_context(&path))
                });
            }
            PlayContextDto::Album(named) => (ContextKind::Album, named),
            PlayContextDto::Artist(named) => (ContextKind::Artist, named),
            PlayContextDto::Playlist(named) => (ContextKind::Playlist, named),
        };
        match named.source {
            SourceKind::Local => self.browse.read_library(|view| {
                let view = view?;
                let tags = view.library.tags();
                Some(match kind {
                    ContextKind::Album => history::local_album_context(
                        tags,
                        browse::local::album_id(&view, &named.id).ok()?,
                    ),
                    ContextKind::Artist => history::local_artist_context(
                        tags,
                        browse::local::artist_id(&view, &named.id).ok()?,
                    ),
                    ContextKind::Playlist => history::local_playlist_context(
                        view.library,
                        browse::local::playlist_id(&view, &named.id).ok()?,
                    ),
                    ContextKind::Folder => return None,
                })
            }),
            SourceKind::Navidrome => {
                browse::check_id(&named.id).ok()?;
                let cover_art = named
                    .cover_url
                    .as_deref()
                    .and_then(|url| self.covers.key_for_url(url))
                    .and_then(|key| match key {
                        coda_core::cover::CoverKey::Navidrome(id) => Some(id),
                        coda_core::cover::CoverKey::Local(_) => None,
                    });
                Context {
                    source: SourceKind::Navidrome,
                    server: self.server.clone(),
                    kind,
                    id: named.id,
                    name: named.name.unwrap_or_default(),
                    subtitle: named.subtitle,
                    cover_art,
                }
                .sanitized()
            }
        }
    }

    /// The context of a `playLocal` intent.
    pub(super) fn local_context(&self, context: &LocalContextDto) -> Option<Context> {
        match context {
            LocalContextDto::Folder { path } => {
                self.play_context(PlayContextDto::Folder { path: path.clone() })
            }
            LocalContextDto::Artist { id } => {
                self.play_context(PlayContextDto::Artist(NamedContextDto {
                    source: SourceKind::Local,
                    id: id.clone(),
                    name: None,
                    subtitle: None,
                    cover_url: None,
                }))
            }
        }
    }

    /// The queue was just replaced by a play from `context` (or by
    /// something that is no context, `None`): record the play, and
    /// remember the entries for the listen counts.
    pub(super) fn played_context(&mut self, context: Option<Context>, now: Instant) {
        self.played_from = context.map(|context| {
            self.history
                .record_play(context.clone(), history::unix_now());
            PlayedFrom {
                context,
                entries: self.player.queue().entries().iter().map(|e| e.id).collect(),
            }
        });
        self.history_changed(now);
    }

    // ----- listens ------------------------------------------------------

    /// The album context of `track`, for its listen count.
    fn album_context(&self, track: &Track) -> Option<Context> {
        match &track.source {
            TrackSource::Navidrome { .. } => Context {
                source: SourceKind::Navidrome,
                server: self.server.clone(),
                kind: ContextKind::Album,
                id: track.album_id.clone()?,
                name: track.album.clone().unwrap_or_default(),
                subtitle: track.album_artist.clone().or_else(|| track.artist.clone()),
                cover_art: track.cover_art.clone(),
            }
            .sanitized(),
            TrackSource::Local { .. } => {
                track.album.as_deref().filter(|a| !a.trim().is_empty())?;
                self.browse.read_library(|view| {
                    let tags = view?.library.tags();
                    Some(history::local_album_context(tags, tags.album_of(track)?))
                })
            }
        }
    }

    /// The current load started: count its listening time.
    pub(super) fn heard_started(&mut self, load: LoadId) {
        if self.player.current_load() != Some(load)
            || self.heard.as_ref().is_some_and(|h| h.listen.load() == load)
        {
            return;
        }
        let Some(entry) = self.player.queue().current_entry() else {
            return;
        };
        let mut contexts: Vec<Context> = self.album_context(&entry.track).into_iter().collect();
        if let Some(from) = &self.played_from
            && from.entries.contains(&entry.id)
        {
            contexts.push(from.context.clone());
        }
        self.heard = (!contexts.is_empty()).then(|| Heard {
            listen: Listen::new(load, String::new(), SystemTime::now()),
            contexts,
        });
    }

    /// A position report: counts the listen once enough was heard.
    pub(super) fn heard_progressed(&mut self, position: Duration, now: Instant) {
        let status = self.player.status();
        if !matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
            return;
        }
        let current_load = self.player.current_load();
        let duration = self
            .player
            .queue()
            .current_entry()
            .and_then(|e| e.track.duration)
            .filter(|d| !d.is_zero())
            .or(self.player.duration());
        let Some(heard) = self
            .heard
            .as_mut()
            .filter(|h| Some(h.listen.load()) == current_load)
        else {
            return;
        };
        if heard
            .listen
            .on_position(position, status == PlaybackStatus::Paused, duration)
            .is_some()
        {
            let contexts = heard.contexts.clone();
            self.history.record_listen(contexts, history::unix_now());
            self.history_changed(now);
        }
    }

    /// A new load, a stop or a backend exit ends the counted listen.
    pub(super) fn heard_ended(&mut self) {
        if self
            .heard
            .as_ref()
            .is_some_and(|h| Some(h.listen.load()) != self.player.current_load())
        {
            self.heard = None;
        }
    }
}

// ----- the loop's writer ------------------------------------------------

/// History writes, one at a time on a blocking thread.
pub(crate) struct HistorySaver {
    pub path: Option<PathBuf>,
    in_flight: Option<JoinHandle<Option<u64>>>,
}

impl HistorySaver {
    pub fn new() -> Self {
        Self {
            path: None,
            in_flight: None,
        }
    }

    fn spawn_write(path: PathBuf, revision: u64, file: HistoryFile) -> JoinHandle<Option<u64>> {
        tokio::task::spawn_blocking(move || match history::save(&path, &file) {
            Ok(()) => Some(revision),
            Err(error) => {
                tracing::warn!(error = %error.kind(), "could not save the play history");
                None
            }
        })
    }

    pub fn poll(&mut self, engine: &mut Engine, now: Instant) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(task) = &self.in_flight {
            if !task.is_finished() {
                return;
            }
            if let Some(Ok(result)) = self.in_flight.take().and_then(futures_now) {
                engine.history_saved(result, now);
            }
        }
        if let Some((revision, file)) = engine.take_due_history(now) {
            self.in_flight = Some(Self::spawn_write(path.clone(), revision, file));
        }
    }

    /// The exit write (the caller bounds it).
    pub fn finish(&mut self, engine: &mut Engine) -> impl Future<Output = ()> + use<> {
        let path = self.path.take();
        let in_flight = self.in_flight.take();
        let last = engine.take_final_history();
        async move {
            if let Some(task) = in_flight {
                let _ = task.await;
            }
            if let (Some(path), Some((revision, file))) = (path, last) {
                let _ = Self::spawn_write(path, revision, file).await;
            }
        }
    }
}
