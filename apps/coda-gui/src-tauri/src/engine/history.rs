//! The engine's side of the play history ([`crate::history`]): context
//! plays recorded from the play intents, listens counted with Coda's
//! scrobble rule (half the track or 4 minutes, [`Listen`]) for every
//! source (per context and per track), where the context that fills the
//! queue stopped, debounced saves, and the Home and Library commands'
//! requests.
//!
//! **Where it stopped** (Home's Resume): while the queue is a context's
//! ([`PlayedFrom`], set by the play intents and a saved mix), the entry
//! that plays and the position are recorded ([`Engine::note_stop`]) when
//! a track starts, on pause, just before another play replaces or clears
//! the queue, and at quit; a context played to its end loses its stop
//! (Resume then starts it over). A play that resumes (`seekMs`) seeks once
//! its first load has started ([`PendingSeek`]).

use std::collections::HashMap;

use super::*;
use crate::browse::dto::LocalContextDto;
use crate::dto::{NamedContextDto, PlayContextDto, PlayingContextDto};
use crate::history::moods::Listens;
use crate::history::{
    self, Context, ContextKind, HeardTrack, History, HistoryFile, HomeRows, Mood, Scope, SongInfo,
    SourceKind, Stop, TrackCount,
};
use coda_core::navidrome::scrobble::Listen;

/// The listen counted for the history (any source).
#[derive(Debug, Clone)]
pub(crate) struct Heard {
    listen: Listen,
    contexts: Vec<Context>,
    /// The track's own count (`None`: not countable, e.g. another
    /// server's entry).
    track: Option<HeardTrack>,
    /// A Navidrome track's genres, kept on its album's count.
    genres: Vec<String>,
}

/// The context the queue was last played from, and its entries.
#[derive(Debug, Clone)]
pub(crate) struct PlayedFrom {
    context: Context,
    /// Each entry's place among the context's tracks in the context's own
    /// order (not the queue's: a shuffled play still says the album's
    /// track number).
    entries: HashMap<QueueEntryId, usize>,
    /// How many tracks the context has (more than queued when a large
    /// local context was cut to fit the queue).
    total: usize,
    /// How the UI names it (Home's Now Playing).
    playing: PlayingContextDto,
    /// The last load that started was one of the context's entries.
    last_started_in: bool,
}

/// A play's tracks as queued: each one's place in the context's own order
/// (in queue order) and the context's size. Set by the play intents for
/// [`Engine::played_context`].
#[derive(Debug, Clone, Default)]
pub(crate) struct QueuedOrder {
    pub positions: Vec<usize>,
    pub total: usize,
}

/// A resumed play's position, sent once its load has started, if that
/// load is the stopped track.
#[derive(Debug, Clone)]
pub(crate) struct PendingSeek {
    load: LoadId,
    seconds: f64,
    /// The stopped track as the UI names it (a local id, a song id).
    track: String,
}

/// A resumed position this close to a track's end (or beyond) is not
/// sought: the track would end at once.
pub(crate) const RESUME_END_MARGIN: Duration = Duration::from_secs(5);

/// What the moods command needs from the history.
#[derive(Debug, Clone)]
pub struct MoodsRequest {
    pub scope: Scope,
    /// The day's moods, if already picked.
    pub cached: Option<Vec<Mood>>,
    /// Album listens by the album's context id.
    pub listens: HashMap<String, Listens>,
    /// Navidrome: the heard albums' genres with their listens.
    pub heard: Vec<(Vec<String>, Listens)>,
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
    /// What the moods of a source and date need (`None`: Navidrome
    /// without a server).
    Moods(SourceKind, String, oneshot::Sender<Option<MoodsRequest>>),
    /// Caches a date's moods picked for a scope; answers the day's moods
    /// (the first stay), or `None` when that server is no longer the
    /// current one.
    SetMoods(Scope, String, Vec<Mood>, oneshot::Sender<Option<Vec<Mood>>>),
    /// The local track listens (key → listens, last listen).
    LocalTrackListens(oneshot::Sender<HashMap<String, (u64, u64)>>),
    /// The current server's most listened songs of an artist (id, name);
    /// `None` without a server.
    NavidromeTopTracks(String, String, oneshot::Sender<Option<Vec<TrackCount>>>),
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

    pub async fn moods_request(
        &self,
        source: SourceKind,
        date: String,
    ) -> Result<Option<MoodsRequest>, EngineGone> {
        self.history_request(|tx| HistoryMsg::Moods(source, date, tx))
            .await
    }

    pub async fn set_moods(
        &self,
        scope: Scope,
        date: String,
        moods: Vec<Mood>,
    ) -> Result<Option<Vec<Mood>>, EngineGone> {
        self.history_request(|tx| HistoryMsg::SetMoods(scope, date, moods, tx))
            .await
    }

    pub async fn local_track_listens(&self) -> Result<HashMap<String, (u64, u64)>, EngineGone> {
        self.history_request(HistoryMsg::LocalTrackListens).await
    }

    pub async fn navidrome_top_tracks(
        &self,
        artist_id: String,
        name: String,
    ) -> Result<Option<Vec<TrackCount>>, EngineGone> {
        self.history_request(|tx| HistoryMsg::NavidromeTopTracks(artist_id, name, tx))
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
            HistoryMsg::Moods(source, date, reply) => {
                let request = self.scope(source).map(|scope| MoodsRequest {
                    cached: self.history.moods(&scope, &date).map(<[_]>::to_vec),
                    listens: self.history.album_listens(&scope),
                    heard: match scope {
                        Scope::Navidrome(_) => self.history.heard_album_genres(&scope),
                        Scope::Local => Vec::new(),
                    },
                    scope,
                });
                let _ = reply.send(request);
            }
            HistoryMsg::SetMoods(scope, date, moods, reply) => {
                // A Navidrome scope is the server the moods were fetched
                // from; the client may have changed since, so check here,
                // where `self.server` changes.
                let current = match &scope {
                    Scope::Local => true,
                    Scope::Navidrome(server) => self.server.as_deref() == Some(server.as_str()),
                };
                let moods = current.then(|| self.history.set_moods(&scope, date, moods));
                self.history_changed(now);
                let _ = reply.send(moods);
            }
            HistoryMsg::LocalTrackListens(reply) => {
                let _ = reply.send(self.history.local_track_listens());
            }
            HistoryMsg::NavidromeTopTracks(artist_id, name, reply) => {
                let top = self.server.as_deref().map(|server| {
                    self.history.navidrome_top_tracks(
                        server,
                        &artist_id,
                        &name,
                        history::TOP_TRACKS,
                    )
                });
                let _ = reply.send(top);
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
        // The snapshot carries the revision (Home reloads its rows).
        self.dirty = true;
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

    /// The exit write, if anything changed since the last one (where the
    /// playing context stopped recorded first).
    pub fn take_final_history(&mut self) -> Option<(u64, HistoryFile)> {
        self.record_stop(None);
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
    /// valid (a local id of the current library; a Navidrome id), and how
    /// the UI names it.
    pub(super) fn play_context(&self, dto: PlayContextDto) -> Option<(Context, PlayingContextDto)> {
        let (kind, named) = match dto {
            PlayContextDto::Folder { path } => {
                let context = self.browse.read_library(|view| {
                    view?.library.tree().dir(&path)?;
                    Some(history::local_folder_context(&path))
                })?;
                let playing = PlayingContextDto {
                    source: SourceKind::Local,
                    kind: ContextKind::Folder,
                    id: context.id.clone(),
                };
                return Some((context, playing));
            }
            PlayContextDto::Album(named) => (ContextKind::Album, named),
            PlayContextDto::Artist(named) => (ContextKind::Artist, named),
            PlayContextDto::Playlist(named) => (ContextKind::Playlist, named),
        };
        let playing = PlayingContextDto {
            source: named.source,
            kind,
            id: named.id.clone(),
        };
        let context = match named.source {
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
                    ContextKind::Folder | ContextKind::Mix => return None,
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
        }?;
        Some((context, playing))
    }

    /// The context of a `playLocal` intent.
    pub(super) fn local_context(
        &self,
        context: &LocalContextDto,
    ) -> Option<(Context, PlayingContextDto)> {
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
            // Genres are not in the play history (genres brief).
            LocalContextDto::Genre { .. } => None,
        }
    }

    /// The history context of saved mix `mix`, now that it fills the
    /// queue (a Navidrome mix with its first song's cover).
    pub(super) fn mix_context(&self, mix: &coda_core::mix::Mix) -> (Context, PlayingContextDto) {
        let (source, server) = match &mix.source {
            coda_core::mix::MixSource::Local => (SourceKind::Local, None),
            coda_core::mix::MixSource::Navidrome { server } => {
                (SourceKind::Navidrome, Some(server.clone()))
            }
        };
        let cover_art = match source {
            SourceKind::Navidrome => self
                .player
                .queue()
                .entries()
                .iter()
                .find_map(|e| e.track.cover_art.clone()),
            SourceKind::Local => None,
        };
        let id = mix.id.to_string();
        (
            Context {
                source,
                server,
                kind: ContextKind::Mix,
                id: id.clone(),
                name: mix.name.clone(),
                subtitle: None,
                cover_art,
            },
            PlayingContextDto {
                source,
                kind: ContextKind::Mix,
                id,
            },
        )
    }

    /// The queue was just replaced by a play from `context` (or by
    /// something that is no context, `None`): record the play, and
    /// remember the entries (with their places in the context, from the
    /// play's [`QueuedOrder`]) for the listen counts and the stops.
    pub(super) fn played_context(
        &mut self,
        context: Option<(Context, PlayingContextDto)>,
        now: Instant,
    ) {
        let order = self.queued_order.take();
        self.played_from = context.map(|(context, playing)| {
            self.history
                .record_play(context.clone(), history::unix_now());
            self.context_of_queue(context, playing, order)
        });
        self.history_changed(now);
    }

    /// The context of the queue as it is now (each entry's place from
    /// `order` when it matches the queue, else its queue position).
    fn context_of_queue(
        &self,
        context: Context,
        playing: PlayingContextDto,
        order: Option<QueuedOrder>,
    ) -> PlayedFrom {
        let entries = self.player.queue().entries();
        let order = order.filter(|o| o.positions.len() == entries.len());
        let total = order.as_ref().map_or(entries.len(), |o| o.total);
        PlayedFrom {
            context,
            entries: entries
                .iter()
                .enumerate()
                .map(|(i, e)| (e.id, order.as_ref().map_or(i, |o| o.positions[i])))
                .collect(),
            total,
            playing,
            last_started_in: false,
        }
    }

    /// A saved mix restored with the session owns the restored queue:
    /// it is the queue's context again (not a new play).
    pub(super) fn adopt_mix_context(&mut self, mix: &coda_core::mix::Mix) {
        let (context, playing) = self.mix_context(mix);
        self.played_from = Some(self.context_of_queue(context, playing, None));
        self.dirty = true;
    }

    /// The context whose queue is playing (an entry of it is the
    /// player's current one and loaded; a saved mix while it owns the
    /// queue), as the UI names it. A context played to its end (nothing
    /// loaded) is not playing.
    pub(super) fn playing_context(&self) -> Option<PlayingContextDto> {
        let from = self.played_from.as_ref()?;
        if from.context.kind == ContextKind::Mix {
            return self
                .playing_mix_id()
                .filter(|id| *id == from.context.id)
                .map(|_| from.playing.clone());
        }
        self.player.current_load()?;
        let entry = self.player.queue().current_entry()?;
        from.entries
            .contains_key(&entry.id)
            .then(|| from.playing.clone())
    }

    // ----- where it stopped ---------------------------------------------

    /// Records where the context that fills the queue is: its current
    /// entry and `position` (none: the player's). Nothing for a mix, an
    /// entry that is not the context's, one that cannot be keyed, or
    /// when nothing is loaded: after the end of the queue the player
    /// keeps `current` on the last entry, and recording it then would
    /// undo [`Self::context_finished`] (LESSONS). Returns whether
    /// anything was recorded.
    fn record_stop(&mut self, position: Option<Duration>) -> bool {
        if self.player.current_load().is_none() {
            return false;
        }
        let Some(from) = &self.played_from else {
            return false;
        };
        if from.context.kind == ContextKind::Mix {
            return false;
        }
        let Some(entry) = self.player.queue().current_entry() else {
            return false;
        };
        let Some(&index) = from.entries.get(&entry.id) else {
            return false;
        };
        let Some(track) = self.heard_track(entry.id, &entry.track) else {
            return false;
        };
        let position = position
            .or_else(|| self.player.position())
            .unwrap_or_default();
        let context = &from.context;
        let stop = Stop {
            source: context.source,
            server: context.server.clone(),
            kind: context.kind,
            id: context.id.clone(),
            track: track.id,
            index,
            total: from.total,
            position_ms: u64::try_from(position.as_millis()).unwrap_or(u64::MAX),
            at: history::unix_now(),
        };
        let before = self.history.revision();
        self.history.record_stop(stop);
        self.history.revision() != before
    }

    /// [`Self::record_stop`], and a save when it changed something.
    pub(super) fn note_stop(&mut self, position: Option<Duration>, now: Instant) {
        if self.record_stop(position) {
            self.history_changed(now);
        }
    }

    /// The context was played through: no stop any more (Resume starts it
    /// over).
    fn forget_stop(&mut self, now: Instant) {
        if let Some(from) = &self.played_from {
            let key = from.context.key();
            self.history.clear_stop(&key);
            self.history_changed(now);
        }
    }

    /// The queue's last entry played to its end: if it is the context's,
    /// the context is finished.
    fn context_finished(&mut self, now: Instant) {
        let ended = self.played_from.as_ref().is_some_and(|from| {
            self.player
                .queue()
                .current_entry()
                .is_some_and(|e| from.entries.contains_key(&e.id))
        });
        if ended {
            self.forget_stop(now);
        }
    }

    /// The current entry, as the UI names its track (a local library id
    /// of this generation, a song id).
    fn current_ui_track(&self) -> Option<String> {
        let entry = self.player.queue().current_entry()?;
        match &entry.track.source {
            TrackSource::Navidrome { song_id } => Some(song_id.clone()),
            TrackSource::Local { path } => self.browse.read_library(|view| {
                let view = view?;
                let id = view.library.track_id_by_path(path)?;
                Some(browse::local_id(view.generation, id.0))
            }),
        }
    }

    /// The seek a resumed play's started load gets: only for the stopped
    /// track, and not at (or past) its end.
    fn resume_seek(&self, seek: PendingSeek) -> Option<f64> {
        if self.current_ui_track().as_deref() != Some(seek.track.as_str()) {
            return None;
        }
        let duration = self
            .player
            .queue()
            .current_entry()
            .and_then(|e| e.track.duration)
            .filter(|d| !d.is_zero())
            .or(self.player.duration());
        if let Some(duration) = duration
            && Duration::from_secs_f64(seek.seconds) + RESUME_END_MARGIN > duration
        {
            return None;
        }
        Some(seek.seconds)
    }

    /// After a playback event: a track of the context started (its stop
    /// moves there, at the resumed position if it resumes), playback
    /// moved on from the context's last entry to one outside it, paused
    /// (the position), or the queue played to its end.
    pub(super) fn stop_after_event(
        &mut self,
        started: Option<LoadId>,
        before: PlaybackStatus,
        now: Instant,
    ) -> Vec<PlayerCommand> {
        let mut commands = Vec::new();
        if let Some(load) = started
            && self.player.current_load() == Some(load)
        {
            let seek = self
                .pending_seek
                .take()
                .filter(|s| s.load == load)
                .and_then(|s| self.resume_seek(s));
            self.context_moved_on(now);
            let position = seek.map_or(Duration::ZERO, Duration::from_secs_f64);
            self.note_stop(Some(position), now);
            if let Some(seconds) = seek {
                commands = self.player.seek_absolute(seconds);
            }
        }
        let status = self.player.status();
        if status == PlaybackStatus::Paused && before != PlaybackStatus::Paused {
            self.note_stop(None, now);
        }
        if status == PlaybackStatus::Stopped
            && before != PlaybackStatus::Stopped
            && self.player.stopped_at_end()
        {
            self.context_finished(now);
        }
        commands
    }

    /// A load started: if it is not the context's, the one before was,
    /// and none of the context's entries is left after it in the queue,
    /// the context was played through.
    fn context_moved_on(&mut self, now: Instant) {
        let Some(from) = self.played_from.as_mut() else {
            return;
        };
        let queue = self.player.queue();
        let Some(current) = queue.current() else {
            return;
        };
        let inside = from.entries.contains_key(&current);
        let was_inside = std::mem::replace(&mut from.last_started_in, inside);
        if inside || !was_inside || from.context.kind == ContextKind::Mix {
            return;
        }
        let rest = queue
            .entries()
            .iter()
            .skip_while(|e| e.id != current)
            .any(|e| from.entries.contains_key(&e.id));
        if !rest {
            self.forget_stop(now);
        }
    }

    /// A play that resumes: once the load it started is heard, and only
    /// if it is `track` (the stopped one, as the UI names it), seek to
    /// `seek_ms`.
    pub(super) fn resume_at(&mut self, seek_ms: Option<u64>, track: Option<String>) {
        self.pending_seek = seek_ms
            .filter(|ms| *ms > 0)
            .zip(track)
            .zip(self.player.current_load())
            .map(|((ms, track), load)| PendingSeek {
                load,
                seconds: ms as f64 / 1000.0,
                track,
            });
    }

    /// A new load that is not the resumed one drops its seek.
    pub(super) fn drop_stale_seek(&mut self) {
        if self
            .pending_seek
            .as_ref()
            .is_some_and(|s| Some(s.load) != self.player.current_load())
        {
            self.pending_seek = None;
        }
    }

    // ----- listens ------------------------------------------------------

    /// The album context of a queue entry, for its listen count. None for
    /// a Navidrome entry queued before the last server change (its album
    /// id is another server's).
    fn album_context(&self, entry: QueueEntryId, track: &Track) -> Option<Context> {
        match &track.source {
            TrackSource::Navidrome { .. } if self.other_server_entries.contains(&entry) => None,
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

    /// A queue entry's own listen count: a local track by its tag key, a
    /// Navidrome song by its id and the current server (none for an entry
    /// queued before the last server change).
    fn heard_track(&self, entry: QueueEntryId, track: &Track) -> Option<HeardTrack> {
        match &track.source {
            TrackSource::Navidrome { .. } if self.other_server_entries.contains(&entry) => None,
            TrackSource::Navidrome { song_id } => HeardTrack {
                source: SourceKind::Navidrome,
                server: self.server.clone(),
                id: song_id.clone(),
                song: Some(SongInfo {
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    album_artist: track.album_artist.clone(),
                    album_id: track.album_id.clone(),
                    artist_id: track.artist_id.clone(),
                    cover_art: track.cover_art.clone(),
                    duration_ms: track.duration.map(|d| d.as_millis() as u64),
                    track_no: track.track_no,
                    disc_no: track.disc_no,
                }),
            }
            .sanitized(),
            TrackSource::Local { .. } => self.browse.read_library(|view| {
                let tags = view?.library.tags();
                let album = tags.album_of(track)?;
                Some(HeardTrack {
                    source: SourceKind::Local,
                    server: None,
                    // `None` without an album tag: not counted.
                    id: history::local_track_key(&history::local_album_key(tags, album), track)?,
                    song: None,
                })
            }),
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
        let mut contexts: Vec<Context> = self
            .album_context(entry.id, &entry.track)
            .into_iter()
            .collect();
        if let Some(from) = &self.played_from
            && from.entries.contains_key(&entry.id)
        {
            contexts.push(from.context.clone());
        }
        let track = self.heard_track(entry.id, &entry.track);
        let genres = match entry.track.source {
            TrackSource::Navidrome { .. } => entry.track.genres.clone(),
            TrackSource::Local { .. } => Vec::new(),
        };
        self.heard = (!contexts.is_empty() || track.is_some()).then(|| Heard {
            listen: Listen::new(load, String::new(), SystemTime::now()),
            contexts,
            track,
            genres,
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
            let track = heard.track.clone();
            let genres = heard.genres.clone();
            let at = history::unix_now();
            // The album's genres (Navidrome) on its count, for the moods.
            let album = contexts
                .iter()
                .find(|c| c.kind == ContextKind::Album)
                .map(Context::key);
            self.history.record_listen(contexts, at);
            if let Some(album) = album {
                self.history.set_album_genres(&album, genres);
            }
            if let Some(track) = track {
                self.history.record_track_listen(track, at);
            }
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
