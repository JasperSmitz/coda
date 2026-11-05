//! The engine actor: one Tokio task owns the player, mpv, the Navidrome
//! client, the library scan, scrobbling and session saving, like Coda's
//! single `select!` loop (`../coda/src/app.rs`). Tauri commands talk to it
//! through an [`EngineHandle`]; it reports through a [`Sink`].
//!
//! [`Engine`] is the synchronous core (no I/O, testable without a window
//! or mpv): intents and playback events go in, backend commands come out.
//! [`run`] is the loop around it: it owns the channels, the
//! [`PlaybackHandle`], the timers and the blocking session writes.
//!
//! Every intent goes through `Player` and `resolve::dispatch` (which also
//! re-announces the gapless next track after queue edits); the UI never
//! sends raw playback commands.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use coda_core::library::Library;
use coda_core::library::scan::{self, ScanEvent};
use coda_core::model::{Track, TrackMetadata, TrackSource};
use coda_core::navidrome::scrobble::{Listen, ScrobbleRequest, Scrobbler};
use coda_core::navidrome::{self, Client};
use coda_core::playback::{LoadId, PlaybackCommand, PlaybackEvent, PlaybackHandle};
use coda_core::player::{PlaybackStatus, Player, PlayerCommand, RepeatMode};
use coda_core::queue::{MAX_QUEUE_LEN, Placement, QueueEntryId, Rng, shuffle_with_first};
use coda_core::resolve;
use coda_core::session::{self, Restored, Session};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::bootstrap::{Sources, Startup};
use crate::cover::CoverService;
use crate::dto::*;

/// Snapshot coalescing and timer resolution.
pub const TICK: Duration = Duration::from_millis(50);
/// At most this often a `progress` event.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Quiet time after the last change before the session is saved.
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);
/// Bound on the final session save at exit.
pub const SAVE_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on mpv's shutdown at exit.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a notice is shown.
pub const NOTICE_TTL: Duration = Duration::from_secs(5);
/// Notices kept at once (older ones are dropped).
const MAX_NOTICES: usize = 4;
/// Bound on waiting for a still-running startup at exit.
const STARTUP_GRACE: Duration = Duration::from_secs(3);
const VISUALIZER_UNAVAILABLE: &str = "The visualizer is not available with this mpv";

/// Where the engine's output goes (Tauri events in the app, a recorder in
/// tests).
pub trait Sink: Send + 'static {
    fn snapshot(&self, snapshot: &Snapshot);
    fn progress(&self, progress: &ProgressDto);
    fn visualizer(&self, frame: &VisualizerDto);
}

enum Msg {
    Intent(Intent),
    Snapshot(oneshot::Sender<Snapshot>),
    Shutdown(oneshot::Sender<()>),
    /// A saved settings change to apply live (see [`settings`]).
    Settings(SettingsChange, oneshot::Sender<Result<(), String>>),
}

/// The way in: cheap to clone, used by Tauri commands and the exit hook.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<Msg>,
}

/// The engine task has ended (the app is quitting).
#[derive(Debug, thiserror::Error)]
#[error("the engine has stopped")]
pub struct EngineGone;

impl EngineHandle {
    pub async fn intent(&self, intent: Intent) -> Result<(), EngineGone> {
        self.tx
            .send(Msg::Intent(intent))
            .await
            .map_err(|_| EngineGone)
    }

    /// The full current state (queue included).
    pub async fn snapshot(&self) -> Result<Snapshot, EngineGone> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Msg::Snapshot(tx))
            .await
            .map_err(|_| EngineGone)?;
        rx.await.map_err(|_| EngineGone)
    }

    /// Applies a settings change that was saved to the config. Fails while
    /// starting (the change takes effect at the next start then) or quitting.
    pub async fn apply_settings(&self, change: SettingsChange) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Msg::Settings(change, tx))
            .await
            .map_err(|_| EngineGone.to_string())?;
        rx.await.map_err(|_| EngineGone.to_string())?
    }

    /// Final session save, then mpv shutdown (both bounded). Returns when
    /// done, or at once if the engine has already stopped.
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Shutdown(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    NoLibrary,
    Scanning,
    Finished { skipped: usize, errors: usize },
    Failed,
}

#[derive(Debug, Clone)]
struct Notice {
    id: u64,
    text: String,
    expires_at: Instant,
}

/// The synchronous core. See the module docs.
pub struct Engine {
    phase: Phase,
    player: Player,
    client: Option<Client>,
    covers: Arc<CoverService>,
    library: Option<Library>,
    scan: ScanState,
    /// Report Navidrome plays (configured, and a client exists).
    scrobbling: bool,
    listen: Option<Listen>,
    scrobbles: Vec<ScrobbleRequest>,
    scrobble_failure_shown: bool,
    /// A view showing the visualizer is visible.
    visualizer_visible: bool,
    /// What mpv was last told.
    visualizer_sent: bool,
    visualizer_unavailable: bool,
    frames: Vec<VisualizerDto>,
    /// Startup problems (config, password, mpv).
    problems: Vec<ProblemDto>,
    navidrome: NavidromeDto,
    notices: VecDeque<Notice>,
    next_notice: u64,
    rng: Rng,
    // Session saving (revision based, like the TUI's `App`).
    seen_revision: u64,
    saved_revision: Option<u64>,
    save_due: Option<Instant>,
    // Emission.
    dirty: bool,
    sent: Option<Snapshot>,
    sent_queue: Option<Vec<QueueEntryId>>,
    sent_progress: Option<(ProgressDto, Instant)>,
    // Live settings (see [`settings`]).
    /// The load playing when the client was swapped: its listen belonged
    /// to the old server and is not reported to the new one.
    unscrobbled_load: Option<LoadId>,
    /// The file's `scrobble` setting (scrobbling also needs a client).
    scrobble_setting: bool,
    client_change: Option<ClientChange>,
    rescan: Option<PathBuf>,
}

impl Engine {
    pub fn new(covers: Arc<CoverService>) -> Self {
        Self {
            phase: Phase::Starting,
            player: Player::new(),
            client: None,
            covers,
            library: None,
            scan: ScanState::NoLibrary,
            scrobbling: false,
            listen: None,
            scrobbles: Vec::new(),
            scrobble_failure_shown: false,
            visualizer_visible: false,
            visualizer_sent: false,
            visualizer_unavailable: false,
            frames: Vec::new(),
            problems: Vec::new(),
            navidrome: NavidromeDto {
                state: ConnectionDto::NotConfigured,
                message: None,
            },
            notices: VecDeque::new(),
            next_notice: 1,
            rng: Rng::from_entropy(),
            seen_revision: 0,
            saved_revision: Some(0),
            save_due: None,
            dirty: true,
            sent: None,
            sent_queue: None,
            sent_progress: None,
            unscrobbled_load: None,
            scrobble_setting: false,
            client_change: None,
            rescan: None,
        }
    }

    // ----- startup -----------------------------------------------------

    /// Applies what startup produced: sources, the playback problem (if
    /// mpv did not start) and the saved session, which is restored without
    /// playing. Returns the commands for mpv (volume, visualizer).
    pub fn start(
        &mut self,
        sources: Sources,
        playback: Option<ProblemDto>,
        restored: Restored,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        self.problems = sources.problems;
        self.covers.set_enabled(sources.cover_art);
        self.covers.set_client(sources.client.clone());
        self.navidrome = if sources.client.is_some() {
            NavidromeDto {
                state: ConnectionDto::Connecting,
                message: None,
            }
        } else if let Some(problem) = self
            .problems
            .iter()
            .find(|p| p.kind == ProblemKind::Navidrome)
        {
            NavidromeDto {
                state: ConnectionDto::Failed,
                message: Some(problem.message.clone()),
            }
        } else {
            NavidromeDto {
                state: ConnectionDto::NotConfigured,
                message: None,
            }
        };
        self.client = sources.client;
        self.scrobble_setting = sources.scrobble;
        self.scrobbling = sources.scrobble && self.client.is_some();
        if let Some(dir) = sources.music_dir {
            self.library = Some(Library::new(dir));
            self.scan = ScanState::Scanning;
        }
        if let Some(problem) = playback {
            // Playback is unavailable from the start; the banner says why,
            // so the player's own message is not repeated as a notice.
            self.player
                .handle_event(PlaybackEvent::BackendExited(problem.message.clone()));
            self.player.take_messages();
            self.problems.push(problem);
        }
        let session = restored.session;
        let commands = self.player.restore(
            session.tracks,
            session.current,
            session.repeat,
            session.volume,
        );
        let mut commands = resolve::dispatch(&mut self.player, self.client.as_ref(), commands);
        if let Some(notice) = restored.notice {
            self.notify(notice, now);
        }
        for message in self.player.take_messages() {
            self.notify(message, now);
        }
        self.phase = Phase::Ready;
        commands.extend(self.sync_visualizer());
        // The restored state counts as saved.
        self.seen_revision = self.player.revision();
        self.saved_revision = Some(self.seen_revision);
        self.save_due = None;
        self.dirty = true;
        commands
    }

    /// The music folder to scan, once started.
    pub fn music_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().map(|l| l.root_path().to_path_buf())
    }

    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    pub fn scrobbling(&self) -> bool {
        self.scrobbling
    }

    // ----- intents -----------------------------------------------------

    pub fn handle_intent(&mut self, intent: Intent, now: Instant) -> Vec<PlaybackCommand> {
        match intent {
            Intent::SetVisualizerVisible { visible } => {
                self.visualizer_visible = visible;
                self.dirty = true;
                return self.sync_visualizer().into_iter().collect();
            }
            Intent::DismissNotice { id } => {
                self.notices.retain(|n| n.id != id);
                self.dirty = true;
                return Vec::new();
            }
            _ if self.phase != Phase::Ready => return Vec::new(),
            _ => {}
        }
        let commands = match intent {
            Intent::TogglePause => self.player.toggle_pause(),
            Intent::Next => self.player.play_next(),
            Intent::Previous => self.player.play_previous(),
            Intent::SeekTo { seconds } => self.player.seek_absolute(seconds),
            Intent::SeekBy { seconds } => self.player.seek_relative(seconds),
            Intent::SetVolume { volume } => self.player.set_volume(volume),
            Intent::CycleRepeat => {
                self.player.cycle_repeat();
                Vec::new()
            }
            Intent::PlayEntry { key } => match self.entry_id(&key) {
                Some(id) => self.player.play_entry(id),
                None => Vec::new(),
            },
            Intent::RemoveEntry { key } => match self.entry_id(&key) {
                Some(id) => self.player.remove(id),
                None => Vec::new(),
            },
            Intent::ClearQueue => self.player.clear_queue(),
            Intent::PlayTracks {
                tracks,
                start,
                shuffle,
            } => self.play_tracks(tracks, start, shuffle, now),
            Intent::Enqueue { tracks, next } => {
                self.enqueue(tracks, next, now);
                Vec::new()
            }
            Intent::SetVisualizerVisible { .. } | Intent::DismissNotice { .. } => Vec::new(),
        };
        let mut commands = self.dispatch(commands, now);
        commands.extend(self.sync_visualizer());
        commands
    }

    fn entry_id(&self, key: &str) -> Option<QueueEntryId> {
        self.player
            .queue()
            .entries()
            .iter()
            .find(|e| e.id.to_string() == key)
            .map(|e| e.id)
    }

    /// The tracks `refs` name, in order; `None` for a local track not in
    /// the scanned library.
    fn resolve_refs(&self, refs: Vec<TrackRef>) -> Vec<Option<Track>> {
        refs.into_iter()
            .map(|r| match r.source {
                SourceDto::Local { path } => {
                    let library = self.library.as_ref()?;
                    let id = library.track_id_by_path(Path::new(&path))?;
                    Some(library.track(id).clone())
                }
                SourceDto::Navidrome { song_id, cover_art } => Some(Track::navidrome(
                    song_id,
                    TrackMetadata {
                        title: r.title,
                        artist: r.artist,
                        album: r.album,
                        album_artist: r.album_artist,
                        track_no: r.track_no,
                        disc_no: r.disc_no,
                        duration: r.duration_ms.map(Duration::from_millis),
                        cover_art,
                    },
                )),
            })
            .collect()
    }

    fn report_missing(&mut self, missing: usize, now: Instant) {
        if missing > 0 {
            self.notify(
                format!("{} not in the library; skipped", count(missing, "track")),
                now,
            );
        }
    }

    /// Replaces the queue (like the TUI's `replace_queue`).
    fn play_tracks(
        &mut self,
        refs: Vec<TrackRef>,
        start: Option<usize>,
        shuffle: bool,
        now: Instant,
    ) -> Vec<PlayerCommand> {
        let resolved = self.resolve_refs(refs);
        let missing = resolved.iter().filter(|t| t.is_none()).count();
        self.report_missing(missing, now);
        // The start's place among the tracks that resolved; a missing start
        // track starts at the next one that did.
        let start = start.map(|s| resolved.iter().take(s).filter(|t| t.is_some()).count());
        let mut tracks: Vec<Track> = resolved.into_iter().flatten().collect();
        if tracks.is_empty() {
            if missing == 0 {
                self.notify("Nothing to play", now);
            }
            return Vec::new();
        }
        let start = start.filter(|&s| s < tracks.len());
        let from = if shuffle {
            shuffle_with_first(&mut tracks, start, &mut self.rng);
            0
        } else {
            start.unwrap_or(0)
        };
        let dropped = tracks.len().saturating_sub(MAX_QUEUE_LEN);
        let commands = self.player.play_context(tracks, from);
        if !commands.is_empty() && dropped > 0 {
            self.notify(
                format!(
                    "Queue full; {} not added",
                    coda_core::model::thousands(dropped)
                ),
                now,
            );
        }
        commands
    }

    fn enqueue(&mut self, refs: Vec<TrackRef>, next: bool, now: Instant) {
        let resolved = self.resolve_refs(refs);
        let missing = resolved.iter().filter(|t| t.is_none()).count();
        self.report_missing(missing, now);
        let tracks: Vec<Track> = resolved.into_iter().flatten().collect();
        if tracks.is_empty() {
            return;
        }
        let wanted = tracks.len();
        let placement = if next {
            Placement::Next
        } else {
            Placement::End
        };
        let added = self.player.enqueue_all(tracks, placement);
        let mut text = match (added, next) {
            (0, _) => String::new(),
            (n, true) => format!("Playing next: {}", count(n, "track")),
            (n, false) => format!("Added {} to the queue", count(n, "track")),
        };
        if added < wanted {
            if !text.is_empty() {
                text.push_str("; ");
            }
            text.push_str(&format!(
                "queue full, {} not added",
                coda_core::model::thousands(wanted - added)
            ));
        }
        self.notify(text, now);
    }

    // ----- playback events ---------------------------------------------

    pub fn handle_playback_event(
        &mut self,
        event: PlaybackEvent,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        match &event {
            PlaybackEvent::Visualizer { load, frame } => {
                // Readings for the track heard, while it plays and a view
                // shows them; the UI smooths and decays the bars.
                if self.visualizer_sent
                    && self.player.audible_load() == Some(*load)
                    && self.player.status() == PlaybackStatus::Playing
                {
                    self.frames.push(VisualizerDto {
                        levels: frame.levels,
                        bands: frame.bands,
                    });
                }
                return Vec::new();
            }
            PlaybackEvent::VisualizerUnavailable => {
                if !self.visualizer_unavailable {
                    self.visualizer_unavailable = true;
                    self.notify(VISUALIZER_UNAVAILABLE, now);
                }
                return Vec::new();
            }
            _ => {}
        }
        let started = match event {
            PlaybackEvent::Started { load } => Some(load),
            _ => None,
        };
        let position = match event {
            PlaybackEvent::Position(position) => Some(position),
            _ => None,
        };
        let commands = self.player.handle_event(event);
        if let Some(load) = started {
            self.listen_started(load);
        }
        if let Some(position) = position {
            self.listen_progressed(position);
        }
        self.dispatch(commands, now)
    }

    /// Resolves the player's commands (and re-syncs the gapless next) and
    /// updates what depends on the player.
    fn dispatch(&mut self, commands: Vec<PlayerCommand>, now: Instant) -> Vec<PlaybackCommand> {
        let commands = resolve::dispatch(&mut self.player, self.client.as_ref(), commands);
        // A new load, a stop or a backend exit ends the current listen.
        if self
            .listen
            .as_ref()
            .is_some_and(|l| Some(l.load()) != self.player.current_load())
        {
            self.listen = None;
        }
        for message in self.player.take_messages() {
            self.notify(message, now);
        }
        self.note_changes(now);
        self.dirty = true;
        commands
    }

    /// `SetVisualizer` if what mpv should do changed: readings only while
    /// a view shows them and the backend supports them.
    fn sync_visualizer(&mut self) -> Option<PlaybackCommand> {
        let wanted = self.visualizer_visible
            && !self.visualizer_unavailable
            && self.phase == Phase::Ready
            && self.player.status() != PlaybackStatus::Unavailable;
        (wanted != self.visualizer_sent).then(|| {
            self.visualizer_sent = wanted;
            PlaybackCommand::SetVisualizer(wanted)
        })
    }

    /// Visualizer readings to emit, oldest first.
    pub fn take_frames(&mut self) -> Vec<VisualizerDto> {
        std::mem::take(&mut self.frames)
    }

    // ----- scrobbling (ported from the TUI's `App`) --------------------

    /// The current load started: a remote track is reported as now
    /// playing, and its listening time counted.
    fn listen_started(&mut self, load: LoadId) {
        if !self.scrobbling
            || self.player.current_load() != Some(load)
            || self.unscrobbled_load == Some(load)
            || self.listen.as_ref().is_some_and(|l| l.load() == load)
        {
            return;
        }
        let song_id = match self.player.queue().current_entry().map(|e| &e.track.source) {
            Some(TrackSource::Navidrome { song_id }) => song_id.clone(),
            _ => return,
        };
        // Wall clock: an `Instant` stops during sleep.
        let listen = Listen::new(load, song_id, SystemTime::now());
        self.scrobbles.push(listen.now_playing());
        self.listen = Some(listen);
    }

    /// A position report for the current load: submits the play once
    /// enough of it was heard.
    fn listen_progressed(&mut self, position: Duration) {
        let status = self.player.status();
        if !matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
            return;
        }
        let current_load = self.player.current_load();
        // The server's duration first (what scrobble services judge by);
        // zero means unknown, then mpv's.
        let duration = self
            .player
            .queue()
            .current_entry()
            .and_then(|e| e.track.duration)
            .filter(|d| !d.is_zero())
            .or(self.player.duration());
        let Some(listen) = self
            .listen
            .as_mut()
            .filter(|l| Some(l.load()) == current_load)
        else {
            return;
        };
        if let Some(request) =
            listen.on_position(position, status == PlaybackStatus::Paused, duration)
        {
            self.scrobbles.push(request);
        }
    }

    pub fn take_scrobbles(&mut self) -> Vec<ScrobbleRequest> {
        std::mem::take(&mut self.scrobbles)
    }

    /// The first scrobble failure of the run is shown; the worker logs all.
    pub fn handle_scrobble_failure(&mut self, error: navidrome::Error, now: Instant) {
        if !self.scrobble_failure_shown {
            self.scrobble_failure_shown = true;
            self.notify(format!("Navidrome scrobbling failed: {error}"), now);
        }
    }

    // ----- Navidrome connection ------------------------------------------

    pub fn handle_ping(&mut self, result: Result<(), navidrome::Error>) {
        self.navidrome = match result {
            Ok(()) => NavidromeDto {
                state: ConnectionDto::Connected,
                message: None,
            },
            Err(error) => {
                tracing::warn!(detail = %error.detail(), "Navidrome ping failed");
                NavidromeDto {
                    state: ConnectionDto::Failed,
                    message: Some(error.to_string()),
                }
            }
        };
        self.dirty = true;
    }

    // ----- library scan ------------------------------------------------

    pub fn handle_scan_event(&mut self, event: ScanEvent, now: Instant) {
        let Some(library) = self.library.as_mut() else {
            return;
        };
        match event {
            ScanEvent::Batch(tracks) => library.extend(tracks),
            ScanEvent::Playlist(file) => {
                if library.add_playlist(file).is_none() {
                    tracing::debug!("playlist over the cap ignored");
                }
            }
            ScanEvent::Finished {
                tracks,
                skipped,
                errors,
            } => {
                tracing::info!(tracks, skipped, errors, "library scan finished");
                self.scan = ScanState::Finished { skipped, errors };
                let playlists = library.playlists().len();
                let mut text = format!("Library scan complete: {}", count(tracks, "track"));
                if playlists > 0 {
                    text.push_str(&format!(", {}", count(playlists, "playlist")));
                }
                if skipped > 0 {
                    text.push_str(&format!(", {skipped} skipped"));
                }
                if errors > 0 {
                    text.push_str(&format!(", {}", count(errors, "error")));
                }
                self.notify(text, now);
            }
        }
        self.dirty = true;
    }

    /// The scan channel closed; without `Finished` first, the scan failed.
    pub fn scan_closed(&mut self, now: Instant) {
        if self.scan == ScanState::Scanning {
            let tracks = self.library.as_ref().map_or(0, Library::len);
            tracing::warn!(tracks, "library scan ended without finishing");
            self.scan = ScanState::Failed;
            self.notify(
                format!(
                    "Library scan failed after {}; see the log",
                    count(tracks, "track")
                ),
                now,
            );
        }
    }

    // ----- notices and timers ------------------------------------------

    fn notify(&mut self, text: impl Into<String>, now: Instant) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        // The same message again just stays up longer.
        self.notices.retain(|n| n.text != text);
        while self.notices.len() >= MAX_NOTICES {
            self.notices.pop_front();
        }
        self.notices.push_back(Notice {
            id: self.next_notice,
            text,
            expires_at: now + NOTICE_TTL,
        });
        self.next_notice += 1;
        self.dirty = true;
    }

    /// Expires notices.
    pub fn tick(&mut self, now: Instant) {
        let before = self.notices.len();
        self.notices.retain(|n| n.expires_at > now);
        if self.notices.len() != before {
            self.dirty = true;
        }
    }

    // ----- session saving ----------------------------------------------

    fn note_changes(&mut self, now: Instant) {
        let revision = self.player.revision();
        if revision != self.seen_revision {
            self.seen_revision = revision;
            self.save_due = Some(now + SAVE_DEBOUNCE);
        }
    }

    /// A snapshot to write once [`SAVE_DEBOUNCE`] passed since the last
    /// change.
    pub fn take_due_save(&mut self, now: Instant) -> Option<Session> {
        if self.save_due.is_none_or(|due| now < due) {
            return None;
        }
        self.save_due = None;
        Some(self.save_snapshot())
    }

    /// The exit save, if anything changed since the last one (or the last
    /// write failed).
    pub fn take_final_save(&mut self) -> Option<Session> {
        self.save_due = None;
        (self.phase != Phase::Starting && self.saved_revision != Some(self.player.revision()))
            .then(|| self.save_snapshot())
    }

    pub fn save_failed(&mut self) {
        self.saved_revision = None;
    }

    fn save_snapshot(&mut self) -> Session {
        self.saved_revision = Some(self.player.revision());
        // The GUI's visualizer follows its views; nothing to restore.
        Session::from_player(&self.player)
    }

    pub fn set_shutting_down(&mut self) {
        self.phase = Phase::ShuttingDown;
        self.dirty = true;
    }

    // ----- state for the UI --------------------------------------------

    fn track_dto(&self, track: &Track) -> TrackDto {
        let source = match &track.source {
            TrackSource::Local { path } => SourceDto::Local {
                path: path.to_string_lossy().into_owned(),
            },
            TrackSource::Navidrome { song_id } => SourceDto::Navidrome {
                song_id: song_id.clone(),
                cover_art: track.cover_art.clone(),
            },
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
            cover_url: self.covers.url_for(track),
        }
    }

    fn problems(&self) -> Vec<ProblemDto> {
        let mut problems = self.problems.clone();
        let playback_known = problems
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::MpvMissing | ProblemKind::Playback));
        if let Some(reason) = self.player.unavailable_reason()
            && !playback_known
        {
            problems.push(ProblemDto {
                kind: ProblemKind::Playback,
                message: format!("Playback stopped: {reason}. Restart Coda to play again."),
            });
        }
        if self.navidrome.state == ConnectionDto::Failed
            && !problems.iter().any(|p| p.kind == ProblemKind::Navidrome)
        {
            problems.push(ProblemDto {
                kind: ProblemKind::Navidrome,
                message: format!(
                    "Navidrome is unreachable: {}",
                    self.navidrome.message.as_deref().unwrap_or("unknown error")
                ),
            });
        }
        problems
    }

    fn scan_dto(&self) -> ScanDto {
        let tracks = self.library.as_ref().map_or(0, Library::len);
        let playlists = self.library.as_ref().map_or(0, |l| l.playlists().len());
        let (state, skipped, errors) = match self.scan {
            ScanState::NoLibrary => (ScanStateDto::NoLibrary, 0, 0),
            ScanState::Scanning => (ScanStateDto::Scanning, 0, 0),
            ScanState::Finished { skipped, errors } => (ScanStateDto::Finished, skipped, errors),
            ScanState::Failed => (ScanStateDto::Failed, 0, 0),
        };
        ScanDto {
            state,
            tracks,
            playlists,
            skipped,
            errors,
        }
    }

    fn queue_dto(&self) -> Vec<QueueItemDto> {
        self.player
            .queue()
            .entries()
            .iter()
            .map(|e| QueueItemDto {
                key: e.id.to_string(),
                track: self.track_dto(&e.track),
            })
            .collect()
    }

    /// The snapshot without the queue.
    fn base_snapshot(&self) -> Snapshot {
        let queue = self.player.queue();
        let playing = self.player.playing_entry();
        Snapshot {
            phase: self.phase,
            status: match self.player.status() {
                PlaybackStatus::Stopped => StatusDto::Stopped,
                PlaybackStatus::Loading => StatusDto::Loading,
                PlaybackStatus::Playing => StatusDto::Playing,
                PlaybackStatus::Paused => StatusDto::Paused,
                PlaybackStatus::Unavailable => StatusDto::Unavailable,
            },
            current_key: queue.current().map(|id| id.to_string()),
            playing_key: playing.map(|e| e.id.to_string()),
            up_next_key: self.player.up_next().map(|e| e.id.to_string()),
            playing: playing.map(|e| self.track_dto(&e.track)),
            queue: None,
            volume: self.player.volume(),
            repeat: match self.player.repeat() {
                RepeatMode::Off => RepeatDto::Off,
                RepeatMode::All => RepeatDto::All,
                RepeatMode::One => RepeatDto::One,
            },
            scan: self.scan_dto(),
            navidrome: self.navidrome.clone(),
            problems: self.problems(),
            notices: self
                .notices
                .iter()
                .map(|n| NoticeDto {
                    id: n.id,
                    text: n.text.clone(),
                })
                .collect(),
            visualizer_available: !self.visualizer_unavailable,
        }
    }

    /// The full state, for the `snapshot` command. The next event then
    /// carries the queue again, so a UI that loads the snapshot while
    /// events race it still converges.
    pub fn full_snapshot(&mut self) -> Snapshot {
        self.sent_queue = None;
        self.sent = None;
        self.dirty = true;
        Snapshot {
            queue: Some(self.queue_dto()),
            ..self.base_snapshot()
        }
    }

    /// The `snapshot` event to send, if anything changed since the last
    /// one. The queue is included only when its entries changed.
    pub fn take_snapshot_event(&mut self) -> Option<Snapshot> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let base = self.base_snapshot();
        let ids: Vec<QueueEntryId> = self.player.queue().entries().iter().map(|e| e.id).collect();
        let queue_changed = self.sent_queue.as_ref() != Some(&ids);
        if !queue_changed && self.sent.as_ref() == Some(&base) {
            return None;
        }
        let queue = queue_changed.then(|| self.queue_dto());
        self.sent_queue = Some(ids);
        self.sent = Some(base.clone());
        Some(Snapshot { queue, ..base })
    }

    /// The `progress` event to send, if the position changed and the last
    /// one is at least [`PROGRESS_INTERVAL`] old.
    pub fn take_progress(&mut self, now: Instant) -> Option<ProgressDto> {
        let progress = ProgressDto {
            position_ms: self.player.position().map(|d| d.as_millis() as u64),
            duration_ms: self.player.duration().map(|d| d.as_millis() as u64),
        };
        if let Some((sent, at)) = self.sent_progress
            && (sent == progress || now.duration_since(at) < PROGRESS_INTERVAL)
        {
            return None;
        }
        self.sent_progress = Some((progress, now));
        Some(progress)
    }
}

fn count(n: usize, noun: &str) -> String {
    let n_text = coda_core::model::thousands(n);
    if n == 1 {
        format!("{n_text} {noun}")
    } else {
        format!("{n_text} {noun}s")
    }
}

// ----- the loop ----------------------------------------------------------

/// Starts the engine task. `startup` produces the sources, mpv and the
/// session (see [`crate::bootstrap::start`]); it runs concurrently, so the
/// UI gets a `Starting` snapshot at once.
pub fn spawn<S, F>(sink: S, covers: Arc<CoverService>, startup: F) -> EngineHandle
where
    S: Sink,
    F: Future<Output = Startup> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(256);
    tauri::async_runtime::spawn(run(rx, sink, covers, startup));
    EngineHandle { tx }
}

/// Waits for `once` (pends forever once taken or absent).
async fn next_once<T>(once: &mut Option<oneshot::Receiver<T>>) -> Option<T> {
    match once {
        Some(rx) => rx.await.ok(),
        None => std::future::pending().await,
    }
}

async fn next_scan(scan: &mut Option<mpsc::Receiver<ScanEvent>>) -> Option<ScanEvent> {
    match scan {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_playback(playback: &mut Option<PlaybackHandle>) -> Option<PlaybackEvent> {
    match playback {
        Some(handle) => handle.recv().await,
        None => std::future::pending().await,
    }
}

async fn startup_done(startup: &mut Option<JoinHandle<Startup>>) -> Option<Startup> {
    match startup {
        Some(task) => task.await.ok(),
        None => std::future::pending().await,
    }
}

/// Session writes, one at a time on a blocking thread, so an older
/// snapshot can never land after a newer one.
struct Saver {
    path: Option<PathBuf>,
    in_flight: Option<JoinHandle<bool>>,
}

impl Saver {
    fn spawn_write(path: PathBuf, session: Session) -> JoinHandle<bool> {
        tokio::task::spawn_blocking(move || match session::save(&path, &session) {
            Ok(()) => {
                tracing::debug!(entries = session.tracks.len(), "session saved");
                true
            }
            Err(error) => {
                tracing::warn!(error = %error.kind(), "could not save the session");
                false
            }
        })
    }

    /// Collects a finished write and starts a due one.
    fn poll(&mut self, engine: &mut Engine, now: Instant) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(task) = &self.in_flight {
            if !task.is_finished() {
                return;
            }
            let task = self.in_flight.take();
            if let Some(Some(Ok(false) | Err(_))) = task.map(futures_now) {
                engine.save_failed();
            }
        }
        if let Some(session) = engine.take_due_save(now) {
            self.in_flight = Some(Self::spawn_write(path.clone(), session));
        }
    }

    /// The exit save, bounded by [`SAVE_TIMEOUT`].
    async fn finish(&mut self, engine: &mut Engine) {
        let Some(path) = self.path.take() else {
            return;
        };
        let in_flight = self.in_flight.take();
        let work = async {
            if let Some(task) = in_flight
                && !matches!(task.await, Ok(true))
            {
                engine.save_failed();
            }
            if let Some(session) = engine.take_final_save()
                && !matches!(Self::spawn_write(path, session).await, Ok(true))
            {
                engine.save_failed();
            }
        };
        if tokio::time::timeout(SAVE_TIMEOUT, work).await.is_err() {
            tracing::warn!("session save did not finish in time");
        }
    }
}

/// The result of a finished task, without waiting.
fn futures_now<T>(task: JoinHandle<T>) -> Option<Result<T, tokio::task::JoinError>> {
    use std::task::{Context, Poll, Waker};
    let mut task = std::pin::pin!(task);
    match task.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    }
}

fn spawn_ping(client: Client) -> oneshot::Receiver<Result<(), navidrome::Error>> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(client.ping().await);
    });
    rx
}

/// Everything the loop owns besides the engine.
struct Io<S> {
    sink: S,
    playback: Option<PlaybackHandle>,
    scan: Option<mpsc::Receiver<ScanEvent>>,
    ping: Option<oneshot::Receiver<Result<(), navidrome::Error>>>,
    scrobbler: Option<Scrobbler>,
    scrobble_failure: Option<oneshot::Receiver<navidrome::Error>>,
    saver: Saver,
}

impl<S: Sink> Io<S> {
    fn send(&self, commands: Vec<PlaybackCommand>) {
        if commands.is_empty() {
            return;
        }
        if let Some(playback) = &self.playback {
            playback.send_all(commands);
        }
    }

    /// Emits what is due and hands scrobbles to the worker.
    fn flush(&mut self, engine: &mut Engine, now: Instant) {
        for frame in engine.take_frames() {
            self.sink.visualizer(&frame);
        }
        for request in engine.take_scrobbles() {
            if let Some(scrobbler) = &self.scrobbler {
                scrobbler.send(request);
            }
        }
        if let Some(snapshot) = engine.take_snapshot_event() {
            self.sink.snapshot(&snapshot);
        }
        if let Some(progress) = engine.take_progress(now) {
            self.sink.progress(&progress);
        }
    }
}

async fn run<S, F>(mut rx: mpsc::Receiver<Msg>, sink: S, covers: Arc<CoverService>, startup: F)
where
    S: Sink,
    F: Future<Output = Startup> + Send + 'static,
{
    let mut engine = Engine::new(covers);
    let mut startup = Some(tokio::spawn(startup));
    let mut io = Io {
        sink,
        playback: None,
        scan: None,
        ping: None,
        scrobbler: None,
        scrobble_failure: None,
        saver: Saver {
            path: None,
            in_flight: None,
        },
    };
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    io.flush(&mut engine, Instant::now());

    let reply = loop {
        tokio::select! {
            started = startup_done(&mut startup) => {
                startup = None;
                let now = Instant::now();
                match started {
                    Some(started) => apply_startup(&mut engine, &mut io, started, now),
                    None => {
                        tracing::error!("startup panicked");
                        let sources = Sources {
                            music_dir: None,
                            client: None,
                            scrobble: false,
                            cover_art: true,
                            problems: Vec::new(),
                        };
                        let problem = ProblemDto {
                            kind: ProblemKind::Playback,
                            message: "Startup failed; see the log".into(),
                        };
                        let restored = Restored {
                            session: Session::default(),
                            save_path: None,
                            notice: None,
                        };
                        engine.start(sources, Some(problem), restored, now);
                    }
                }
            }
            msg = rx.recv() => match msg {
                Some(Msg::Intent(intent)) => {
                    let commands = engine.handle_intent(intent, Instant::now());
                    io.send(commands);
                }
                Some(Msg::Snapshot(reply)) => {
                    let _ = reply.send(engine.full_snapshot());
                }
                Some(Msg::Shutdown(reply)) => break Some(reply),
                Some(Msg::Settings(change, reply)) => {
                    let result = engine.apply_settings(change, Instant::now());
                    let _ = reply.send(result.as_ref().map(drop).map_err(Clone::clone));
                    if let Ok(commands) = result {
                        io.send(commands);
                    }
                    apply_settings_io(&mut engine, &mut io);
                }
                None => break None,
            },
            event = next_playback(&mut io.playback), if io.playback.is_some() => match event {
                Some(event) => {
                    let commands = engine.handle_playback_event(event, Instant::now());
                    io.send(commands);
                }
                // The backend has gone (after `BackendExited`); keep the
                // handle for the shutdown call but stop reading.
                None => {
                    if let Some(playback) = io.playback.take() {
                        playback.shutdown(SHUTDOWN_TIMEOUT).await;
                    }
                }
            },
            event = next_scan(&mut io.scan) => match event {
                Some(event) => engine.handle_scan_event(event, Instant::now()),
                None => {
                    io.scan = None;
                    engine.scan_closed(Instant::now());
                }
            },
            result = next_once(&mut io.ping) => {
                io.ping = None;
                if let Some(result) = result {
                    engine.handle_ping(result);
                }
            }
            error = next_once(&mut io.scrobble_failure) => {
                io.scrobble_failure = None;
                if let Some(error) = error {
                    engine.handle_scrobble_failure(error, Instant::now());
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                engine.tick(now);
                io.saver.poll(&mut engine, now);
            }
        }
        // Visualizer frames go out at once; the rest is coalesced by the
        // snapshot comparison (and `progress` by its interval).
        io.flush(&mut engine, Instant::now());
    };

    // Shutdown: tell the UI, final save, then mpv (both bounded).
    engine.set_shutting_down();
    io.flush(&mut engine, Instant::now());
    if let Some(task) = startup.take() {
        // mpv may still be starting; let it finish so it is shut down
        // cleanly, else abort (dropping the handle kills the child).
        match tokio::time::timeout(STARTUP_GRACE, task).await {
            Ok(Ok(started)) => io.playback = started.playback.ok(),
            Ok(Err(_)) => {}
            Err(_) => tracing::warn!("startup still running at exit; abandoned"),
        }
    }
    io.saver.finish(&mut engine).await;
    io.scan = None;
    io.scrobbler = None;
    if let Some(playback) = io.playback.take() {
        let outcome = playback.shutdown(SHUTDOWN_TIMEOUT).await;
        tracing::info!(?outcome, "playback backend shut down");
    }
    if let Some(reply) = reply {
        let _ = reply.send(());
    }
}

fn apply_startup<S: Sink>(engine: &mut Engine, io: &mut Io<S>, started: Startup, now: Instant) {
    let Startup {
        sources,
        playback,
        restored,
    } = started;
    let (handle, problem) = match playback {
        Ok(handle) => (Some(handle), None),
        Err(problem) => (None, Some(problem)),
    };
    io.saver.path = restored.save_path.clone();
    io.playback = handle;
    let commands = engine.start(sources, problem, restored, now);
    io.send(commands);
    if let Some(dir) = engine.music_dir() {
        io.scan = Some(scan::spawn_scan(dir));
    }
    if let Some(client) = engine.client().cloned() {
        io.ping = Some(spawn_ping(client.clone()));
        if engine.scrobbling() {
            let (scrobbler, failure) = Scrobbler::spawn(client);
            io.scrobbler = Some(scrobbler);
            io.scrobble_failure = Some(failure);
        }
    }
}

mod settings;
pub use settings::SettingsChange;
use settings::{ClientChange, apply_settings_io};

#[cfg(test)]
mod tests;
