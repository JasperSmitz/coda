//! Application state and the single `tokio::select!` loop.
//!
//! [`App`] is plain state: user [`Action`]s, [`PlaybackEvent`]s and
//! [`ScanEvent`]s go in, [`PlaybackCommand`]s come out. It never performs
//! I/O and takes the current time as a parameter, so it is tested without
//! a terminal, mpv or a real clock.
//!
//! The player emits loads in terms of a track's source; [`App`] resolves
//! each into a backend target (a local file, or a stream URL signed by the
//! Navidrome client) just before it leaves, see [`crate::resolve`]. A load
//! that cannot be resolved is fed back to the player as a failed track, so
//! the queue advances exactly as for a missing file.
//!
//! The Library panel has two sources: the local library ([`Library`],
//! browsed from its Home with [`LocalBrowser`]: playlists, folders,
//! artists, albums) and, when configured, Navidrome
//! ([`navidrome::browse::Browser`]). Remote lists are fetched in
//! the background: [`App`] queues fetch requests as data
//! ([`App::take_fetch_requests`]) and the loop spawns them, at most
//! [`MAX_CONCURRENT_FETCHES`] at a time; results come back through
//! [`App::handle_fetch_response`].
//!
//! Navidrome playlist edits work the same way: the browser queues
//! [`EditRequest`]s ([`App::take_edit_requests`]), the loop runs each as
//! a sequential job ([`navidrome::edit::spawn_edit`]), one at a time, and
//! the result comes back through [`App::handle_edit_response`].
//!
//! Settings (`,`) are a [`Settings`] overlay: like the browser, it emits
//! requests (ping, save, disconnect) that the loop performs on tasks and
//! blocking threads. A saved or removed Navidrome account is swapped in
//! without touching playback: a new client generation (responses from
//! the old client are ignored), a fresh browser, a new ping and a new
//! scrobbler ([`App::take_client_change`]). Queued remote tracks resolve
//! against whatever client is current at their next load.
//!
//! The session (queue, current entry, volume, repeat mode) is restored at
//! startup ([`App::restore_session`]) and saved by the loop: [`App`] only
//! reports that a save is due ([`App::take_due_save`], debounced by
//! [`SAVE_DEBOUNCE`]) and hands over a snapshot; the loop writes it on a
//! blocking thread.
//!
//! [`run`] owns the terminal lifecycle: it enters the TUI, installs the
//! panic hook, starts the scan, runs the loop, and on every exit saves the
//! session and shuts the playback backend down (both bounded) before
//! restoring the terminal. Only this module and `ui/` use Ratatui or
//! Crossterm.

use std::io;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind,
};
use futures_util::{FutureExt, StreamExt};
use ratatui::DefaultTerminal;
use ratatui::buffer::Buffer;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::library::browse::{self as local, LocalBrowser, Row as LocalRow};
use crate::library::scan::{self, ScanEvent};
use crate::library::search::{self, SearchResults};
use crate::library::{Library, MAX_PLAYLISTS};
use crate::line_input::LineInput;
use crate::model::Track;
use crate::model::TrackSource;
use crate::navidrome;
use crate::navidrome::Hidden;
use crate::navidrome::browse::{
    self, Activated, Browser, FetchRequest, FetchResponse, Generation, JobOutcome,
    MAX_CONCURRENT_FETCHES, Removal, SaveOrder, UNSAVED_ORDER,
};
use crate::navidrome::edit::{EditId, EditRequest, EditResponse};
use crate::navidrome::scrobble::{Listen, ScrobbleRequest, Scrobbler};
use crate::onboarding::Input;
use crate::playback::{LoadId, PlaybackCommand, PlaybackEvent, PlaybackHandle};
use crate::player::{PlaybackStatus, Player, PlayerCommand, QUEUE_FULL};
use crate::queue::{Placement, QueueEntryId, Rng};
use crate::resolve;
use crate::secret_store::{SecretError, SecretStore};
use crate::session::{self, Session};
use crate::settings::{self, Change, Settings};
use crate::setup::{self, Reply, SaveNavidrome};
use crate::ui::{help, keymap, render};
use crate::visualizer::Meter;

/// Redraw interval when nothing else changes (progress, status expiry).
pub const TICK: Duration = Duration::from_millis(250);
/// How long a transient status message stays visible.
pub const STATUS_TTL: Duration = Duration::from_secs(5);
/// Upper bound on waiting for the playback backend at quit.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Seek step for the arrow keys, in seconds.
pub const SEEK_STEP: f64 = 5.0;
/// The session is saved this long after the last change to it.
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);
/// Upper bound on waiting for the session save at quit.
pub const SAVE_TIMEOUT: Duration = Duration::from_secs(2);
/// Upper bound on waiting at exit for a settings save that is running.
pub const SETTINGS_SAVE_WAIT: Duration = Duration::from_secs(5);
/// A second `C` within this time clears the queue.
pub const CLEAR_CONFIRM: Duration = Duration::from_secs(3);
/// Status after the first `C`.
pub const CLEAR_PROMPT: &str = "Press C again to clear the queue";
/// Status for "Add to playlist" (`P`) on local tracks.
pub const LOCAL_TO_PLAYLIST: &str = "Only Navidrome songs can be added to a Navidrome playlist";
/// Status for `S` without Navidrome songs in the queue.
pub const NO_REMOTE_IN_QUEUE: &str = "No Navidrome songs in the queue to save";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Library,
    Queue,
}

/// What the Library panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Local,
    Navidrome,
}

/// Navidrome connection state, as last observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// The startup ping has not answered yet.
    Connecting,
    Connected,
    /// Short, URL-free error text.
    Failed(String),
}

/// Shown for the Navidrome source without a Navidrome configuration.
pub const NAVIDROME_NOT_CONFIGURED: &str = "Navidrome is not configured";
/// Below [`NAVIDROME_NOT_CONFIGURED`]: Enter opens the Connect form.
pub const PRESS_ENTER_TO_CONNECT: &str = "Press Enter to connect";
/// Status when quitting is refused while a playlist change is pending.
pub const SAVING_PLAYLIST: &str = "A playlist change is being saved; press again to quit";
/// Upper bound on waiting at exit for the playlist change that is running.
pub const EDIT_SAVE_WAIT: Duration = Duration::from_secs(5);
/// Status when quitting is refused while settings are being saved.
pub const SAVING_SETTINGS: &str = "Saving settings… press Ctrl-C again to quit";
/// Status when `/` is pressed on the local source without a music
/// directory.
pub const NO_LOCAL_LIBRARY: &str = "No local music directory configured";
/// Status when the visualizer cannot work (shown once, and on `v`).
pub const VISUALIZER_UNAVAILABLE: &str = "Visualizer unavailable";

/// A user intent, already resolved from a key by [`keymap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    /// Enter: open a directory, artist or album, play a library track
    /// now, or play the selected queue entry.
    Activate,
    /// Library: go to the parent directory or previous view.
    Parent,
    /// Library: enqueue the track, all tracks below the directory, or an
    /// album, playlist or artist (local or remote).
    Enqueue,
    /// Library (`A`): like [`Action::Enqueue`], but "play next": after the
    /// current entry, behind earlier "play next" tracks.
    EnqueueNext,
    /// Library (`o`): show an album list (local or Navidrome) in the next
    /// sort order.
    CycleSort,
    /// `1`: show the local library (focuses the Library panel).
    SelectLocal,
    /// `2`: show Navidrome (focuses the Library panel).
    SelectNavidrome,
    /// Library: open the search input (local or Navidrome).
    OpenSearch,
    /// Esc in the library: leave local or Navidrome search results.
    Back,
    /// Library: refetch the shown Navidrome view.
    Refresh,
    /// Search input: a typed character.
    InputChar(char),
    InputBackspace,
    /// Search input: Enter.
    InputSubmit,
    /// Search input: Esc.
    InputCancel,
    /// Queue: remove the selected entry.
    Remove,
    /// Queue: move the selected entry up (`K`); the selection follows it.
    MoveEntryUp,
    /// Queue: move the selected entry down (`J`).
    MoveEntryDown,
    /// Queue: remove every entry (`C`).
    ClearQueue,
    /// Queue: select the current entry (`g`).
    SelectCurrent,
    /// Queue: shuffle the entries after the current one (`s`).
    ShuffleUpcoming,
    /// `R`: cycle repeat off → all → one.
    CycleRepeat,
    TogglePause,
    Next,
    Previous,
    SeekBackward,
    SeekForward,
    VolumeUp,
    VolumeDown,
    SwitchFocus,
    ToggleHelp,
    CloseHelp,
    /// `,`: open the Settings overlay.
    OpenSettings,
    /// Queue (`S`): save the queue's Navidrome songs as a new playlist.
    SaveQueueAsPlaylist,
    /// Library (`P`): add the selection to a Navidrome playlist (picker).
    AddToPlaylist,
    /// Playlists list (`N`): create an empty playlist.
    NewPlaylist,
    /// Playlists list (`e`): rename the selected playlist.
    RenamePlaylist,
    /// Playlists list (`D`, twice): delete the selected playlist.
    DeletePlaylist,
    /// Playlist view (`d`, twice): remove the selected song.
    RemoveFromPlaylist,
    /// Playlist view (`w`): save the changed order.
    SaveOrder,
    /// `v`: show or hide the visualizer.
    ToggleVisualizer,
    Quit,
}

impl Action {
    /// Library-panel actions that have nothing to act on while the
    /// Navidrome source only offers to connect.
    fn is_library_only(self) -> bool {
        matches!(
            self,
            Self::Up
                | Self::Down
                | Self::Parent
                | Self::Back
                | Self::Enqueue
                | Self::EnqueueNext
                | Self::CycleSort
                | Self::OpenSearch
                | Self::Refresh
                | Self::AddToPlaylist
                | Self::NewPlaylist
                | Self::RenamePlaylist
                | Self::DeletePlaylist
                | Self::RemoveFromPlaylist
                | Self::SaveOrder
                | Self::MoveEntryUp
                | Self::MoveEntryDown
        )
    }
}

/// A playlist edit armed by its first key press; the second press within
/// [`CLEAR_CONFIRM`], on the same item and with nothing in between,
/// confirms it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EditArm {
    Delete(String),
    /// Saving an order that drops hidden entries.
    SaveOrder(String),
    Remove {
        playlist_id: String,
        index: usize,
        song_id: String,
    },
}

/// A new Navidrome client (or none) for the loop to start a ping and a
/// scrobbler for; see [`App::take_client_change`].
#[derive(Debug)]
pub struct ClientChange {
    /// The new client's generation, for its ping.
    pub generation: Generation,
    pub client: Option<navidrome::Client>,
    /// Start a scrobbler for it.
    pub scrobble: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Scanning {
        tracks: usize,
    },
    Finished {
        tracks: usize,
        skipped: usize,
        errors: usize,
    },
    /// The scan channel closed without `Finished`: the scan ended early.
    Failed {
        tracks: usize,
    },
    /// No local music directory is configured; nothing is scanned.
    NoLibrary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
    pub expires_at: Instant,
}

/// Local search results, shown in the Library panel instead of the tree
/// until left with `h`/Esc. A snapshot: later scan batches do not change
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSearch {
    pub results: SearchResults,
    /// The scan was still running (or had failed) when the search ran, so
    /// only the tracks scanned so far were searched.
    pub partial: bool,
    /// Selected row in `results.tracks`.
    pub selected: usize,
}

#[derive(Debug)]
pub struct App {
    player: Player,
    /// What the scan has found so far.
    library: Library,
    /// Local browse navigation (Home, folders, artists, albums,
    /// playlists).
    local: LocalBrowser,
    queue_selected: usize,
    focus: Focus,
    help_open: bool,
    /// First help line shown; clamped by the loop to what the last frame
    /// could show ([`App::clamp_help_scroll`]).
    help_scroll: usize,
    /// Until when a second `C` clears the queue.
    clear_armed_until: Option<Instant>,
    /// A playlist delete or removal awaiting its second press, and until
    /// when.
    edit_armed: Option<(EditArm, Instant)>,
    /// The local search input while it is open.
    local_input: Option<LineInput>,
    /// Local search results, while shown.
    local_search: Option<LocalSearch>,
    scan: ScanState,
    status: Option<StatusMessage>,
    quit: bool,
    /// Signs stream URLs for remote tracks; `None` if not configured.
    navidrome: Option<navidrome::Client>,
    source: Source,
    /// Remote browse state; `Some` exactly when Navidrome is configured.
    browser: Option<Browser>,
    connection: Connection,
    /// Drives `s` (shuffle upcoming).
    rng: Rng,
    /// Player revision last seen by [`App::note_changes`].
    seen_revision: u64,
    /// Player revision of the last snapshot handed out for saving; `None`
    /// after a failed save, so the exit save retries.
    saved_revision: Option<u64>,
    /// When the debounced save is due.
    save_due: Option<Instant>,
    /// Report Navidrome plays; only ever `true` with a client.
    scrobbling: bool,
    /// Listening progress of the current load, if it is a remote track
    /// and scrobbling is on.
    listen: Option<Listen>,
    /// Scrobbles to send, in order; see [`App::take_scrobble_requests`].
    scrobbles: Vec<ScrobbleRequest>,
    /// A scrobble failure has been shown; later ones are only logged.
    scrobble_failure_shown: bool,
    /// The Settings overlay.
    settings: Settings,
    /// The current client's generation; fetch responses from another are
    /// ignored.
    generation: Generation,
    /// A client swap the loop has not picked up yet.
    client_change: Option<ClientChange>,
    /// The load current at the last client swap: resolved for the old
    /// server, so it is never reported as a listen to the new one (it may
    /// still be loading when the swap lands).
    unscrobbled_load: Option<LoadId>,
    /// A first quit was refused while settings were being saved; the next
    /// one quits (the exit still waits, bounded, for the save).
    quit_armed: bool,
    /// A first quit was refused while a playlist change was pending; the
    /// next one quits.
    edit_quit_armed: bool,
    /// The loop is running an edit (maintained by its runner). Unlike the
    /// browser's pending edits, it survives an account swap, so an edit
    /// still running for the old account warns at quit too.
    edit_in_flight: bool,
    /// The visualizer is shown (`v`; saved with the session).
    visualizer: bool,
    /// The backend cannot run the visualizer this session.
    visualizer_unavailable: bool,
    /// The last frame had room for the visualizer, uncovered by an
    /// overlay (assumed until a frame says otherwise).
    visualizer_drawable: bool,
    /// What the backend was last told (`SetVisualizer`): on only while
    /// shown and drawable, so nothing is polled for a visualizer nobody
    /// sees.
    visualizer_sent: bool,
    /// The visualizer's bars.
    meter: Meter,
    /// Bumped when an application setting worth saving changes (the
    /// visualizer); added to the player's revision.
    settings_revision: u64,
}

impl App {
    /// An app browsing `music_dir`, without Navidrome.
    pub fn new(music_dir: PathBuf) -> Self {
        Self::with_sources(Some(music_dir), None)
    }

    /// An app with an optional local library and an optional Navidrome
    /// client. Without a music directory the library stays empty and no
    /// scan is expected; if Navidrome is configured it is then the active
    /// source, and its artists are requested right away.
    pub fn with_sources(music_dir: Option<PathBuf>, navidrome: Option<navidrome::Client>) -> Self {
        let scan = match music_dir {
            Some(_) => ScanState::Scanning { tracks: 0 },
            None => ScanState::NoLibrary,
        };
        let navidrome_configured = navidrome.is_some();
        let settings = Settings::new(settings::Context::unsaved(
            music_dir.clone(),
            navidrome.as_ref(),
        ));
        let mut browser = navidrome
            .as_ref()
            .map(|client| Browser::for_user(Generation::default(), client.username()));
        let source = match (&music_dir, &mut browser) {
            (None, Some(browser)) => {
                browser.ensure_started();
                Source::Navidrome
            }
            _ => Source::Local,
        };
        Self {
            player: Player::new(),
            library: Library::new(music_dir.unwrap_or_default()),
            local: LocalBrowser::new(),
            queue_selected: 0,
            focus: Focus::Library,
            help_open: false,
            help_scroll: 0,
            clear_armed_until: None,
            edit_armed: None,
            local_input: None,
            local_search: None,
            scan,
            status: None,
            quit: false,
            navidrome,
            source,
            browser,
            connection: Connection::Connecting,
            rng: Rng::from_entropy(),
            seen_revision: 0,
            saved_revision: Some(0),
            save_due: None,
            scrobbling: navidrome_configured,
            listen: None,
            scrobbles: Vec::new(),
            scrobble_failure_shown: false,
            settings,
            generation: Generation::default(),
            client_change: None,
            unscrobbled_load: None,
            quit_armed: false,
            edit_quit_armed: false,
            edit_in_flight: false,
            visualizer: false,
            visualizer_unavailable: false,
            visualizer_drawable: true,
            visualizer_sent: false,
            meter: Meter::default(),
            settings_revision: 0,
        }
    }

    /// Sets what the Settings overlay shows and may change (see
    /// [`settings::Context`]).
    pub fn set_settings_context(&mut self, context: settings::Context) {
        self.settings = Settings::new(context);
    }

    /// Turns scrobbling of Navidrome plays on or off (on by default when
    /// Navidrome is configured; it stays off without a client).
    pub fn set_scrobbling(&mut self, enabled: bool) {
        self.scrobbling = enabled && self.navidrome.is_some();
        if !self.scrobbling {
            self.listen = None;
        }
    }

    /// Restores a saved session: the queue, its current entry (selected in
    /// the Queue panel), the repeat mode and the visualizer, without
    /// playing anything. Returns the commands that apply the saved volume
    /// (if it differs from the backend's startup volume) and turn the
    /// visualizer on (if it was). The restored state counts as saved.
    pub fn restore_session(&mut self, session: Session, now: Instant) -> Vec<PlaybackCommand> {
        let restored = session.tracks.len();
        let commands = self.player.restore(
            session.tracks,
            session.current,
            session.repeat,
            session.volume,
        );
        let mut commands = resolve::dispatch(&mut self.player, self.navidrome.as_ref(), commands);
        self.visualizer = session.visualizer;
        commands.extend(self.sync_visualizer());
        let queue = self.player.queue();
        self.queue_selected = queue
            .current()
            .and_then(|id| queue.position(id))
            .unwrap_or(0);
        if restored > 0 {
            self.set_status(format!("Restored queue: {}", count(restored, "track")), now);
        }
        self.seen_revision = self.revision();
        self.saved_revision = Some(self.seen_revision);
        self.save_due = None;
        commands
    }

    /// Increases whenever something worth saving changes.
    fn revision(&self) -> u64 {
        self.player.revision() + self.settings_revision
    }

    // ----- session saving ----------------------------------------------

    /// Starts (or restarts) the save debounce if the saved state changed.
    fn note_changes(&mut self, now: Instant) {
        let revision = self.revision();
        if revision != self.seen_revision {
            self.seen_revision = revision;
            self.save_due = Some(now + SAVE_DEBOUNCE);
        }
    }

    /// A snapshot to save, once [`SAVE_DEBOUNCE`] has passed since the last
    /// change. The caller writes it; see [`App::save_failed`].
    pub fn take_due_save(&mut self, now: Instant) -> Option<Session> {
        if self.save_due.is_none_or(|due| now < due) {
            return None;
        }
        self.save_due = None;
        Some(self.save_snapshot())
    }

    /// A snapshot to save at exit, if anything changed since the last one
    /// (or the last save failed). Cancels a pending debounce.
    pub fn take_final_save(&mut self) -> Option<Session> {
        self.save_due = None;
        (self.saved_revision != Some(self.revision())).then(|| self.save_snapshot())
    }

    /// The last snapshot could not be written; the exit save retries.
    pub fn save_failed(&mut self) {
        self.saved_revision = None;
    }

    fn save_snapshot(&mut self) -> Session {
        self.saved_revision = Some(self.revision());
        Session {
            visualizer: self.visualizer,
            ..Session::from_player(&self.player)
        }
    }

    // ----- read access for rendering -----------------------------------

    pub fn player(&self) -> &Player {
        &self.player
    }

    /// The visualizer is to be drawn (shown, and the backend supports it).
    pub fn visualizer_shown(&self) -> bool {
        self.visualizer && !self.visualizer_unavailable
    }

    /// The visualizer's bars; silent unless a track is playing.
    pub fn meter(&self) -> &Meter {
        &self.meter
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Local browse navigation.
    pub fn local_browser(&self) -> &LocalBrowser {
        &self.local
    }

    pub fn music_dir(&self) -> &Path {
        self.library.root_path()
    }

    /// The open folder's path from the music root while a folder is shown;
    /// empty otherwise.
    pub fn library_path(&self) -> &[String] {
        self.local.folder_path()
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    pub fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    /// Keeps the help scroll within what the last frame could show, so
    /// scrolling back up responds at once.
    pub fn clamp_help_scroll(&mut self, max: usize) {
        self.help_scroll = self.help_scroll.min(max);
    }

    /// The number of Navidrome list rows the last frame showed, so paged
    /// lists load ahead by a screen. Ignored while nothing was shown.
    pub fn set_remote_rows(&mut self, rows: usize) {
        if rows > 0
            && let Some(browser) = self.browser.as_mut()
        {
            browser.set_viewport_rows(rows);
        }
    }

    pub fn scan_state(&self) -> ScanState {
        self.scan
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The Navidrome browse state, if Navidrome is configured.
    pub fn browser(&self) -> Option<&Browser> {
        self.browser.as_ref()
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// `true` while the Settings overlay takes all keys.
    pub fn settings_open(&self) -> bool {
        self.settings.is_open()
    }

    /// The Navidrome source is shown without Navidrome: the panel offers
    /// to connect.
    pub fn navidrome_prompt(&self) -> bool {
        self.source == Source::Navidrome && self.browser.is_none()
    }

    /// The current Navidrome client, if any.
    pub fn navidrome_client(&self) -> Option<&navidrome::Client> {
        self.navidrome.as_ref()
    }

    /// `true` while a text input (local or Navidrome search, or a
    /// playlist name) takes all keys.
    pub fn input_open(&self) -> bool {
        self.local_input.is_some()
            || self
                .browser
                .as_ref()
                .is_some_and(|b| b.input().is_some() || b.prompt().is_some())
    }

    /// The local search input while it is open.
    pub fn local_input(&self) -> Option<&str> {
        self.local_input.as_ref().map(LineInput::as_str)
    }

    /// Local search results, while shown.
    pub fn local_search(&self) -> Option<&LocalSearch> {
        self.local_search.as_ref()
    }

    /// Scrobbles to send, in order.
    pub fn take_scrobble_requests(&mut self) -> Vec<ScrobbleRequest> {
        std::mem::take(&mut self.scrobbles)
    }

    /// The next playlist edit to run (at most one at a time).
    pub fn take_edit_requests(&mut self) -> Vec<EditRequest> {
        self.browser
            .as_mut()
            .map(Browser::take_edit_requests)
            .unwrap_or_default()
    }

    /// Fetches to spawn, in order.
    pub fn take_fetch_requests(&mut self) -> Vec<FetchRequest> {
        self.browser
            .as_mut()
            .map(Browser::take_requests)
            .unwrap_or_default()
    }

    fn remote_active(&self) -> bool {
        self.focus == Focus::Library && self.source == Source::Navidrome && self.browser.is_some()
    }

    /// Library keys act on the "Press Enter to connect" prompt.
    fn prompt_active(&self) -> bool {
        self.focus == Focus::Library && self.navidrome_prompt()
    }

    /// Library keys act on local search results.
    fn local_results_active(&self) -> bool {
        self.focus == Focus::Library && self.source == Source::Local && self.local_search.is_some()
    }

    /// The transient message, if it has not expired at `now`.
    pub fn status(&self, now: Instant) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|s| now < s.expires_at)
            .map(|s| s.text.as_str())
    }

    pub fn library_rows(&self) -> Vec<LocalRow<'_>> {
        self.local.rows(&self.library, 0, usize::MAX)
    }

    /// Number of rows in the shown local view.
    pub fn library_row_count(&self) -> usize {
        self.local.len(&self.library)
    }

    /// At most `len` rows starting at row `start`, without building the rest.
    pub fn library_rows_window(&self, start: usize, len: usize) -> Vec<LocalRow<'_>> {
        self.local.rows(&self.library, start, len)
    }

    /// Selected library row index; always in range when there are rows.
    pub fn library_selected(&self) -> usize {
        self.local.selected()
    }

    /// Selected queue row index; always in range when there are entries.
    pub fn queue_selected(&self) -> usize {
        self.queue_selected
    }

    /// Library keys act on the local browser (not search results, and
    /// not while there is no local library to show).
    fn local_browse_active(&self) -> bool {
        self.focus == Focus::Library
            && self.source == Source::Local
            && self.local_search.is_none()
            && self.scan != ScanState::NoLibrary
    }

    // ----- inputs ------------------------------------------------------

    /// Applies a user action and returns the commands to send.
    pub fn handle_action(&mut self, action: Action, now: Instant) -> Vec<PlaybackCommand> {
        if self.settings.is_open() {
            // The loop sends settings keys to `handle_settings_input`.
            if action == Action::Quit {
                self.request_quit(now);
            }
            return Vec::new();
        }
        if let Some(input) = self.local_input.as_mut() {
            match action {
                Action::InputChar(c) => input.push_char(c),
                Action::InputBackspace => input.backspace(),
                Action::InputSubmit => self.submit_local_search(),
                Action::InputCancel => self.local_input = None,
                Action::Quit => self.request_quit(now),
                _ => {}
            }
            return Vec::new();
        }
        if let Some(browser) = self.browser.as_mut().filter(|b| b.input().is_some()) {
            match action {
                Action::InputChar(c) => browser.input_char(c),
                Action::InputBackspace => browser.input_backspace(),
                Action::InputSubmit => browser.submit_search(),
                Action::InputCancel => browser.cancel_search(),
                Action::Quit => self.request_quit(now),
                _ => {}
            }
            return Vec::new();
        }
        if let Some(browser) = self.browser.as_mut().filter(|b| b.prompt().is_some()) {
            match action {
                Action::InputChar(c) => browser.prompt_char(c),
                Action::InputBackspace => browser.prompt_backspace(),
                Action::InputSubmit => {
                    let outcomes = browser.submit_prompt();
                    self.apply_job_outcomes(outcomes, now);
                }
                Action::InputCancel => browser.cancel_prompt(),
                Action::Quit => self.request_quit(now),
                _ => {}
            }
            return Vec::new();
        }
        if let Some(browser) = self.browser.as_mut().filter(|b| b.picker().is_some()) {
            match action {
                Action::Up => browser.picker_move(-1),
                Action::Down => browser.picker_move(1),
                Action::Activate => {
                    let outcomes = browser.picker_choose();
                    self.apply_job_outcomes(outcomes, now);
                }
                Action::Back | Action::Parent => browser.close_picker(),
                Action::Quit => self.request_quit(now),
                _ => {}
            }
            return Vec::new();
        }
        if self.help_open {
            match action {
                Action::ToggleHelp | Action::CloseHelp => self.help_open = false,
                Action::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                Action::Down => {
                    self.help_scroll =
                        (self.help_scroll + 1).min(help::BINDINGS.len().saturating_sub(1));
                }
                Action::Quit => self.request_quit(now),
                _ => {}
            }
            return Vec::new();
        }
        // Any other action cancels a pending `C`, `D` or `d`.
        let clear_armed = self.clear_armed_until.take();
        let edit_armed = self.edit_armed.take();
        let mut backend = Vec::new();
        let commands = match action {
            Action::Activate if self.prompt_active() => {
                self.settings.open_connect();
                Vec::new()
            }
            // Before Up/Down: the hidden local selection must not move.
            _ if self.prompt_active() && action.is_library_only() => Vec::new(),
            Action::Up => {
                self.move_selection(-1);
                Vec::new()
            }
            Action::Down => {
                self.move_selection(1);
                Vec::new()
            }
            Action::Activate if self.remote_active() => self.activate_remote(now),
            Action::Activate if self.local_results_active() => self.play_search_result(now),
            Action::Activate => match self.focus {
                Focus::Library => self.activate_library(now),
                Focus::Queue => self.activate_queue(),
            },
            Action::Parent | Action::Refresh
                if self.remote_active() && self.browser_mut().has_unsaved_order() =>
            {
                self.set_status(UNSAVED_ORDER, now);
                Vec::new()
            }
            Action::Parent => {
                if self.remote_active() {
                    self.browser_mut().go_up();
                } else if self.local_results_active() {
                    self.local_search = None;
                } else if self.local_browse_active() {
                    self.local.go_up(&self.library);
                }
                Vec::new()
            }
            Action::Back => {
                let discarded = if self.remote_active() && !self.local_results_active() {
                    self.browser_mut().discard_order()
                } else {
                    None
                };
                if self.local_results_active() {
                    self.local_search = None;
                } else if let Some(message) = discarded {
                    self.set_status(message, now);
                } else if self.remote_active()
                    && self
                        .browser_mut()
                        .current()
                        .is_some_and(|v| matches!(v.resource, browse::Resource::Search(_)))
                {
                    self.browser_mut().go_up();
                }
                Vec::new()
            }
            Action::Enqueue | Action::EnqueueNext => {
                let placement = if action == Action::EnqueueNext {
                    Placement::Next
                } else {
                    Placement::End
                };
                if self.remote_active() {
                    let outcomes = self.browser_mut().enqueue_selected(placement);
                    self.apply_job_outcomes(outcomes, now);
                } else if self.local_results_active() {
                    self.enqueue_search_result(placement, now);
                } else if self.local_browse_active() {
                    self.enqueue_selected(placement, now);
                }
                Vec::new()
            }
            Action::CycleSort => {
                if self.remote_active() {
                    self.browser_mut().cycle_album_sort();
                } else if self.local_browse_active() {
                    self.local.cycle_album_order(&self.library);
                }
                Vec::new()
            }
            Action::SelectLocal => {
                self.source = Source::Local;
                self.focus = Focus::Library;
                Vec::new()
            }
            Action::SelectNavidrome => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.ensure_started();
                }
                // Without Navidrome the panel offers to connect.
                self.source = Source::Navidrome;
                self.focus = Focus::Library;
                Vec::new()
            }
            Action::OpenSearch => {
                if self.remote_active() && self.browser_mut().has_unsaved_order() {
                    self.set_status(UNSAVED_ORDER, now);
                } else if self.remote_active() {
                    self.browser_mut().open_search();
                } else if self.focus == Focus::Library {
                    if self.scan == ScanState::NoLibrary {
                        self.set_status(NO_LOCAL_LIBRARY, now);
                    } else {
                        self.local_input = Some(LineInput::new());
                    }
                }
                Vec::new()
            }
            Action::Refresh => {
                if self.remote_active() {
                    self.browser_mut().refresh();
                }
                Vec::new()
            }
            Action::InputChar(_)
            | Action::InputBackspace
            | Action::InputSubmit
            | Action::InputCancel => Vec::new(),
            Action::Remove => match self.focus {
                Focus::Queue => self.remove_selected(),
                Focus::Library => Vec::new(),
            },
            Action::MoveEntryUp | Action::MoveEntryDown => {
                let up = action == Action::MoveEntryUp;
                if self.focus == Focus::Queue {
                    self.move_selected_entry(up);
                } else if self.remote_active()
                    && let Err(reason) = self.browser_mut().move_entry(if up { -1 } else { 1 })
                    && !reason.is_empty()
                {
                    self.set_status(reason, now);
                }
                Vec::new()
            }
            Action::SaveQueueAsPlaylist => {
                if self.focus == Focus::Queue {
                    self.save_queue_as_playlist(now);
                }
                Vec::new()
            }
            Action::AddToPlaylist => {
                if self.remote_active() {
                    self.browser_mut().open_picker();
                } else if self.local_results_active() || self.local_browse_active() {
                    self.set_status(LOCAL_TO_PLAYLIST, now);
                }
                Vec::new()
            }
            Action::NewPlaylist => {
                if self.remote_active()
                    && self
                        .browser_mut()
                        .current()
                        .is_some_and(|v| v.resource == browse::Resource::Playlists)
                {
                    self.browser_mut().open_new_playlist();
                }
                Vec::new()
            }
            Action::RenamePlaylist => {
                if self.remote_active()
                    && let Err(reason) = self.browser_mut().open_rename()
                    && !reason.is_empty()
                {
                    self.set_status(reason, now);
                }
                Vec::new()
            }
            Action::DeletePlaylist => {
                if self.remote_active() {
                    self.delete_playlist(edit_armed, now);
                }
                Vec::new()
            }
            Action::RemoveFromPlaylist => {
                if self.remote_active() {
                    self.remove_from_playlist(edit_armed, now);
                }
                Vec::new()
            }
            Action::SaveOrder => {
                if self.remote_active() {
                    self.save_order(edit_armed, now);
                }
                Vec::new()
            }
            Action::ClearQueue => match self.focus {
                Focus::Queue => self.clear_queue(clear_armed, now),
                Focus::Library => Vec::new(),
            },
            Action::SelectCurrent => {
                if self.focus == Focus::Queue {
                    self.select_current(now);
                }
                Vec::new()
            }
            Action::ShuffleUpcoming => {
                if self.focus == Focus::Queue {
                    self.shuffle_upcoming(now);
                }
                Vec::new()
            }
            Action::CycleRepeat => {
                self.player.cycle_repeat();
                Vec::new()
            }
            Action::TogglePause => self.player.toggle_pause(),
            Action::Next => self.player.play_next(),
            Action::Previous => self.player.play_previous(),
            Action::SeekBackward => self.player.seek_relative(-SEEK_STEP),
            Action::SeekForward => self.player.seek_relative(SEEK_STEP),
            Action::VolumeUp => self.player.volume_up(),
            Action::VolumeDown => self.player.volume_down(),
            Action::SwitchFocus => {
                self.focus = match self.focus {
                    Focus::Library => Focus::Queue,
                    Focus::Queue => Focus::Library,
                };
                Vec::new()
            }
            Action::ToggleHelp => {
                self.help_open = true;
                self.help_scroll = 0;
                Vec::new()
            }
            Action::CloseHelp => Vec::new(),
            Action::OpenSettings => {
                self.settings.open();
                Vec::new()
            }
            Action::ToggleVisualizer => {
                backend.extend(self.toggle_visualizer(now));
                Vec::new()
            }
            Action::Quit => {
                self.request_quit(now);
                Vec::new()
            }
        };
        let mut commands = resolve::dispatch(&mut self.player, self.navidrome.as_ref(), commands);
        commands.extend(backend);
        self.after_player_call(now);
        commands
    }

    /// `v`: flips (and saves) the setting. Hidden, the bars drop at once.
    fn toggle_visualizer(&mut self, now: Instant) -> Option<PlaybackCommand> {
        self.visualizer = !self.visualizer;
        self.settings_revision += 1;
        self.meter.reset();
        if self.visualizer && self.visualizer_unavailable {
            self.set_status(VISUALIZER_UNAVAILABLE, now);
        }
        self.sync_visualizer()
    }

    /// Whether the last frame could show the visualizer (enough room, no
    /// overlay on top), whatever the setting. Returns the command that
    /// starts or stops the backend's readings if that changes what it
    /// should do.
    pub fn set_visualizer_drawable(&mut self, drawable: bool) -> Option<PlaybackCommand> {
        self.visualizer_drawable = drawable;
        if !drawable {
            self.meter.reset();
        }
        self.sync_visualizer()
    }

    /// `SetVisualizer` if the backend's state differs from what is wanted.
    fn sync_visualizer(&mut self) -> Option<PlaybackCommand> {
        let wanted = self.visualizer && self.visualizer_drawable;
        (wanted != self.visualizer_sent).then(|| {
            self.visualizer_sent = wanted;
            PlaybackCommand::SetVisualizer(wanted)
        })
    }

    /// A visualizer reading. Returns `true` if the bars changed and the
    /// visualizer should be redrawn; readings are dropped while it is
    /// hidden or the track is not playing (or is not the current one).
    pub fn handle_visualizer(
        &mut self,
        load: LoadId,
        frame: &crate::playback::VisualizerFrame,
        now: Instant,
    ) -> bool {
        let current = self.player.current_load() == Some(load)
            && self.player.status() == PlaybackStatus::Playing;
        if !self.visualizer_shown() || !self.visualizer_drawable || !current {
            return false;
        }
        self.meter.update(load, frame, now);
        true
    }

    pub fn handle_playback_event(
        &mut self,
        event: PlaybackEvent,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        match &event {
            PlaybackEvent::Visualizer { load, frame } => {
                self.handle_visualizer(*load, frame, now);
                return Vec::new();
            }
            PlaybackEvent::VisualizerUnavailable => {
                if !self.visualizer_unavailable {
                    self.visualizer_unavailable = true;
                    self.meter.reset();
                    self.set_status(VISUALIZER_UNAVAILABLE, now);
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
        let commands = resolve::dispatch(&mut self.player, self.navidrome.as_ref(), commands);
        self.after_player_call(now);
        commands
    }

    /// A scrobble failed. The first failure of the session is shown in the
    /// status line; the worker logs every failure.
    pub fn handle_scrobble_failure(&mut self, error: navidrome::Error, now: Instant) {
        if !self.scrobble_failure_shown {
            self.scrobble_failure_shown = true;
            self.set_status(format!("Navidrome scrobbling failed: {error}"), now);
        }
    }

    /// The current load started: if it is a remote track, report it as
    /// now playing and start counting what is listened to.
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
        // Wall clock, not `Instant`: an `Instant` stops during system
        // sleep, so it cannot be turned back into the date of the play.
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
        // The server's duration first: it is what scrobble services judge
        // by, and a stream's reported duration may be an estimate. A zero
        // from the server means unknown, so mpv's value is used then.
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

    /// A ping's result for the client of `generation`; ignored if that
    /// client has since been replaced.
    pub fn handle_ping(
        &mut self,
        generation: Generation,
        result: Result<(), navidrome::Error>,
        now: Instant,
    ) {
        if generation == self.generation {
            self.handle_navidrome_ping(result, now);
        } else {
            tracing::debug!("ping result for an old client ignored");
        }
    }

    /// Shows the result of the current client's Navidrome ping.
    pub fn handle_navidrome_ping(&mut self, result: Result<(), navidrome::Error>, now: Instant) {
        match result {
            Ok(()) => {
                tracing::info!("navidrome ping ok");
                self.connection = Connection::Connected;
                self.set_status("Navidrome connected", now);
            }
            Err(error) => {
                // Never `Debug`: network errors include the server address.
                // Only the detail: `Display` can carry untrusted server text.
                tracing::warn!(endpoint = "ping", detail = %error.detail(), "navidrome ping failed");
                self.connection = Connection::Failed(error.to_string());
                self.set_status(format!("Navidrome: {error}"), now);
            }
        }
    }

    // ----- settings ------------------------------------------------------

    /// A key while the Settings overlay is open. Ctrl-C, and `q` outside
    /// the form, quit, except while a save is running.
    pub fn handle_settings_input(&mut self, input: Input, now: Instant) {
        let quit =
            input == Input::Quit || (input == Input::Char('q') && self.settings.form().is_none());
        if quit {
            self.request_quit(now);
        } else {
            self.settings.handle_input(input, now);
        }
    }

    /// Quits. While settings are being saved, the first request only
    /// warns and the second quits; the exit then still waits (up to
    /// [`SETTINGS_SAVE_WAIT`]) for the save, so the process does not stop
    /// between the keychain write and the config write. Likewise while a
    /// playlist change is queued or running (the exit waits up to
    /// [`EDIT_SAVE_WAIT`] for the running one; queued ones are dropped).
    fn request_quit(&mut self, now: Instant) {
        let saving = matches!(
            self.settings.busy(),
            Some(settings::Busy::Saving | settings::Busy::Disconnecting)
        );
        let editing =
            self.edit_in_flight || self.browser.as_ref().is_some_and(|b| b.pending_edits() > 0);
        if saving && !self.quit_armed {
            self.quit_armed = true;
            self.set_status(SAVING_SETTINGS, now);
        } else if editing && !saving && !self.edit_quit_armed {
            self.edit_quit_armed = true;
            self.set_status(SAVING_PLAYLIST, now);
        } else {
            self.quit = true;
        }
    }

    /// Settings requests to perform, in order.
    pub fn take_settings_requests(&mut self) -> Vec<settings::Request> {
        self.settings.take_requests()
    }

    /// Applies a finished settings request; a saved or removed account is
    /// swapped in at once (see [`App::take_client_change`]).
    pub fn handle_settings_response(&mut self, response: settings::Response, now: Instant) {
        let change = self.settings.handle_response(response);
        if self.settings.busy().is_none() {
            self.quit_armed = false;
        }
        match change {
            Some(Change::Connected(saved)) => {
                // On screen only: the user typed it.
                let label =
                    settings::account_label(saved.client.base_url(), saved.client.username());
                self.swap_client(Some(saved.client), saved.scrobble);
                self.set_status(format!("Navidrome saved; connecting as {label}"), now);
            }
            Some(Change::Disconnected) => {
                self.swap_client(None, false);
                self.set_status("Disconnected from Navidrome", now);
            }
            None => {}
        }
    }

    /// A client swap for the loop: ping the new client and replace the
    /// scrobbler (stopping the old one). `None` until the next swap.
    pub fn take_client_change(&mut self) -> Option<ClientChange> {
        self.client_change.take()
    }

    /// Replaces the Navidrome client without touching playback: the track
    /// playing keeps its stream, and queued remote tracks resolve with the
    /// new client (or fail without one) at their next load. Browsing
    /// starts over under a new generation, so responses still in flight
    /// from the old client are ignored, and a listen in progress belongs
    /// to the old server and is dropped.
    fn swap_client(&mut self, client: Option<navidrome::Client>, scrobble: bool) {
        self.generation = Generation(self.generation.0 + 1);
        self.browser = client
            .as_ref()
            .map(|client| Browser::for_user(self.generation, client.username()));
        match self.browser.as_mut() {
            Some(browser) if self.source == Source::Navidrome => browser.ensure_started(),
            None if self.source == Source::Navidrome && self.scan != ScanState::NoLibrary => {
                self.source = Source::Local;
            }
            _ => {}
        }
        self.navidrome = client;
        self.connection = Connection::Connecting;
        self.listen = None;
        self.unscrobbled_load = self.player.current_load();
        self.scrobbles.clear();
        self.scrobble_failure_shown = false;
        self.scrobbling = scrobble && self.navidrome.is_some();
        self.client_change = Some(ClientChange {
            generation: self.generation,
            client: self.navidrome.clone(),
            scrobble: self.scrobbling,
        });
        tracing::info!(
            generation = self.generation.0,
            navidrome = self.navidrome.is_some(),
            scrobble = self.scrobbling,
            "navidrome client replaced"
        );
    }

    /// Bracketed paste: into the open search input, local or Navidrome,
    /// or the playlist name prompt (through the typed-character filter, so
    /// newlines never submit), ignored otherwise.
    pub fn handle_paste(&mut self, text: &str) {
        if self.settings.is_open() {
            self.settings.handle_paste(text);
        } else if let Some(input) = self.local_input.as_mut() {
            input.push_str(text);
        } else if let Some(browser) = self.browser.as_mut() {
            if browser.prompt().is_some() {
                browser.prompt_str(text);
            } else {
                browser.input_str(text);
            }
        }
    }

    /// Applies a finished playlist edit: shows what it did and reloads the
    /// lists it changed. A result for a replaced client is ignored.
    pub fn handle_edit_response(&mut self, response: EditResponse, now: Instant) {
        if response.generation != self.generation {
            tracing::debug!(id = response.id.0, "edit result for an old client ignored");
            return;
        }
        let Some(browser) = self.browser.as_mut() else {
            return;
        };
        let message = browser.handle_edit_response(response);
        if browser.pending_edits() == 0 && !self.edit_in_flight {
            self.edit_quit_armed = false;
        }
        if let Some(message) = message {
            self.set_status(message, now);
        }
    }

    /// Applies a finished fetch: caches it, updates the view if it still
    /// awaits this request, and completes enqueue jobs that were waiting
    /// for it. A successful fetch also marks the server as connected.
    pub fn handle_fetch_response(&mut self, response: FetchResponse, now: Instant) {
        let Some(browser) = self.browser.as_mut() else {
            return;
        };
        if response.generation != self.generation {
            // From a client that has since been replaced or removed.
            tracing::debug!(
                id = response.id.0,
                "fetch response for an old client ignored"
            );
            return;
        }
        if response.result.is_ok() {
            self.connection = Connection::Connected;
        }
        let outcomes = browser.handle_response(response);
        self.apply_job_outcomes(outcomes, now);
        self.note_changes(now);
    }

    pub fn handle_scan_event(&mut self, event: ScanEvent, now: Instant) {
        match event {
            ScanEvent::Batch(tracks) => {
                let key = self.local.selected_key(&self.library);
                self.library.extend(tracks);
                if let ScanState::Scanning { tracks } = &mut self.scan {
                    *tracks = self.library.len();
                }
                self.local.reselect(&self.library, key);
                self.clamp_selections();
            }
            ScanEvent::Playlist(file) => {
                let key = self.local.selected_key(&self.library);
                if self.library.add_playlist(file).is_none() {
                    tracing::debug!(max = MAX_PLAYLISTS, "playlist over the cap ignored");
                }
                self.local.reselect(&self.library, key);
            }
            ScanEvent::Finished {
                tracks,
                skipped,
                errors,
            } => {
                tracing::info!(tracks, skipped, errors, "library scan finished");
                self.scan = ScanState::Finished {
                    tracks,
                    skipped,
                    errors,
                };
                let mut text = format!("Library scan complete: {}", count(tracks, "track"));
                let playlists = self.library.playlists().len();
                if playlists > 0 {
                    text.push_str(&format!(", {}", count(playlists, "playlist")));
                }
                if skipped > 0 {
                    text.push_str(&format!(", {skipped} skipped"));
                }
                if errors > 0 {
                    text.push_str(&format!(", {}", count(errors, "error")));
                }
                self.set_status(text, now);
            }
        }
    }

    /// The scan channel closed. Without a preceding `Finished`, the scan
    /// ended early (it failed or its thread panicked).
    pub fn scan_closed(&mut self, now: Instant) {
        if let ScanState::Scanning { tracks } = self.scan {
            tracing::warn!(tracks, "library scan ended without finishing");
            self.scan = ScanState::Failed { tracks };
            self.set_status(
                format!(
                    "Library scan failed after {}; see the log",
                    count(tracks, "track")
                ),
                now,
            );
        }
    }

    /// Drops the status message once it has expired. Returns `true` if
    /// anything visible changed.
    /// Also lets the visualizer's bars fall when readings stopped coming
    /// while playing (a stall: buffering, or a seek mpv did not perform).
    pub fn tick(&mut self, now: Instant) -> bool {
        let faded = self.meter.fade(now);
        if self.status.as_ref().is_some_and(|s| now >= s.expires_at) {
            self.status = None;
            return true;
        }
        faded
    }

    pub fn set_status(&mut self, text: impl Into<String>, now: Instant) {
        self.status = Some(StatusMessage {
            text: text.into(),
            expires_at: now + STATUS_TTL,
        });
    }

    // ----- helpers -----------------------------------------------------

    fn meter_load_is_stale(&self) -> bool {
        !self.meter.is_empty() && self.meter.load() != self.player.current_load()
    }

    fn after_player_call(&mut self, now: Instant) {
        // Paused, stopped or another track: the bars drop to silence.
        if self.player.status() != PlaybackStatus::Playing || self.meter_load_is_stale() {
            self.meter.reset();
        }
        // A new load, a stop or a backend exit ends the current listen.
        if self
            .listen
            .as_ref()
            .is_some_and(|l| Some(l.load()) != self.player.current_load())
        {
            self.listen = None;
        }
        if let Some(last) = self.player.take_messages().pop() {
            self.set_status(last, now);
        }
        self.clamp_selections();
        self.note_changes(now);
    }

    fn clamp_selections(&mut self) {
        self.local.clamp(&self.library);
        self.queue_selected = self
            .queue_selected
            .min(self.player.queue().len().saturating_sub(1));
    }

    fn move_selection(&mut self, delta: isize) {
        if self.remote_active() {
            self.browser_mut().move_selection(delta);
            return;
        }
        if self.local_results_active()
            && let Some(search) = self.local_search.as_mut()
        {
            search.selected = search
                .selected
                .saturating_add_signed(delta)
                .min(search.results.tracks.len().saturating_sub(1));
            return;
        }
        match self.focus {
            Focus::Library if self.local_browse_active() => {
                self.local.move_selection(&self.library, delta);
            }
            Focus::Library => {}
            Focus::Queue => {
                self.queue_selected = self
                    .queue_selected
                    .saturating_add_signed(delta)
                    .min(self.player.queue().len().saturating_sub(1));
            }
        }
    }

    fn activate_library(&mut self, now: Instant) -> Vec<PlayerCommand> {
        if !self.local_browse_active() {
            return Vec::new();
        }
        match self.local.activate(&self.library) {
            local::Activated::Play(id) => {
                let track = self.library.track(id).clone();
                self.set_status(format!("Playing {}", track.display_name()), now);
                self.player.play_track_now(track)
            }
            local::Activated::Opened | local::Activated::Nothing => Vec::new(),
        }
    }

    /// Enter in the local search input: runs the search over every track
    /// scanned so far and shows the results. A blank query only closes the
    /// input (results already shown stay).
    fn submit_local_search(&mut self) {
        let Some(input) = self.local_input.take() else {
            return;
        };
        let Some(results) = search::search(self.library.tree(), input.as_str()) else {
            return;
        };
        // A failed scan also left only part of the library.
        let partial = matches!(
            self.scan,
            ScanState::Scanning { .. } | ScanState::Failed { .. }
        );
        tracing::debug!(matches = results.total, partial, "local search");
        self.local_search = Some(LocalSearch {
            results,
            partial,
            selected: 0,
        });
    }

    fn selected_search_result(&self) -> Option<Track> {
        let search = self.local_search.as_ref()?;
        search.results.tracks.get(search.selected).cloned()
    }

    fn play_search_result(&mut self, now: Instant) -> Vec<PlayerCommand> {
        match self.selected_search_result() {
            Some(track) => {
                self.set_status(format!("Playing {}", track.display_name()), now);
                self.player.play_track_now(track)
            }
            None => Vec::new(),
        }
    }

    fn enqueue_search_result(&mut self, placement: Placement, now: Instant) {
        if let Some(track) = self.selected_search_result() {
            self.enqueue_with_status(vec![track], placement, now);
        }
    }

    fn browser_mut(&mut self) -> &mut Browser {
        self.browser
            .as_mut()
            .expect("remote actions require a configured browser")
    }

    fn activate_remote(&mut self, now: Instant) -> Vec<PlayerCommand> {
        match self.browser_mut().activate() {
            Activated::Play(track) => {
                self.set_status(format!("Playing {}", track.display_name()), now);
                self.player.play_track_now(track)
            }
            Activated::Opened | Activated::Nothing => Vec::new(),
        }
    }

    /// Adds completed enqueue jobs to the queue, in order, and reports
    /// them in the status line. Failures take priority over other outcomes
    /// of the same batch, with the number of tracks added alongside.
    fn apply_job_outcomes(&mut self, outcomes: Vec<JobOutcome>, now: Instant) {
        let mut failures = Vec::new();
        let mut added = 0;
        let mut not_added = 0;
        let mut last = None;
        for outcome in outcomes {
            match outcome {
                JobOutcome::Added {
                    label,
                    tracks,
                    placement,
                } => {
                    let verb = self.added_verb(placement);
                    last = Some(match tracks.as_slice() {
                        [] => format!("Nothing to add from {label}"),
                        [one] => format!("{verb} {}", one.display_name()),
                        many => format!("{verb} {} from {label}", count(many.len(), "track")),
                    });
                    let (n, dropped) = self.enqueue_tracks(tracks, placement);
                    added += n;
                    not_added += dropped;
                }
                JobOutcome::Fetching { label } => last = Some(format!("Fetching {label}…")),
                JobOutcome::Queued { label, after } => {
                    last = Some(format!("Queued {label} after {after}…"));
                }
                JobOutcome::Failed { label, error } => {
                    failures.push(format!("Could not add {label}: {error}"));
                }
                JobOutcome::Editing { message } => last = Some(message),
            }
        }
        let text = match failures.as_slice() {
            [] => last,
            [first, rest @ ..] => {
                let mut text = first.clone();
                if !rest.is_empty() {
                    text.push_str(&format!(" (and {} more)", rest.len()));
                }
                if added > 0 {
                    text.push_str(&format!("; added {}", count(added, "track")));
                }
                Some(text)
            }
        };
        let text = match (text, not_added) {
            (text, 0) => text,
            (Some(text), n) => Some(format!("{text}; queue full, {} not added", thousands(n))),
            (None, _) => Some(QUEUE_FULL.to_owned()),
        };
        if let Some(text) = text {
            self.set_status(text, now);
        }
        self.clamp_selections();
    }

    /// Adds `tracks` in order at `placement`, up to the queue's cap.
    /// Returns how many were added and how many were dropped.
    fn enqueue_tracks(&mut self, tracks: Vec<Track>, placement: Placement) -> (usize, usize) {
        let total = tracks.len();
        let added = self.player.enqueue_all(tracks, placement);
        (added, total - added)
    }

    /// How the status line reports tracks added at `placement`: "play
    /// next" without a current entry just appends, so it says so.
    fn added_verb(&self, placement: Placement) -> &'static str {
        match placement {
            Placement::Next if self.player.queue().current().is_some() => "Playing next:",
            _ => "Added",
        }
    }

    fn activate_queue(&mut self) -> Vec<PlayerCommand> {
        match self.player.queue().entries().get(self.queue_selected) {
            Some(entry) => {
                let id = entry.id;
                self.player.play_entry(id)
            }
            None => Vec::new(),
        }
    }

    /// `a`/`A` in the local browser: the selected track, or every track
    /// of the selected folder, artist, album or playlist, in order.
    fn enqueue_selected(&mut self, placement: Placement, now: Instant) {
        let Some(ids) = self.local.selected_tracks(&self.library) else {
            return;
        };
        if ids.is_empty() {
            self.set_status("Nothing to add", now);
            return;
        }
        let tracks = ids
            .into_iter()
            .map(|id| self.library.track(id).clone())
            .collect();
        self.enqueue_with_status(tracks, placement, now);
    }

    /// Adds `tracks` at `placement` and reports what was added in the
    /// status line.
    fn enqueue_with_status(&mut self, tracks: Vec<Track>, placement: Placement, now: Instant) {
        let verb = self.added_verb(placement);
        let text = match tracks.as_slice() {
            [one] => format!("{verb} {}", one.display_name()),
            many => format!("{verb} {}", count(many.len(), "track")),
        };
        let text = match self.enqueue_tracks(tracks, placement) {
            (_, 0) => text,
            (0, _) => QUEUE_FULL.to_owned(),
            (added, dropped) => format!(
                "{verb} {}; queue full, {} not added",
                count(added, "track"),
                thousands(dropped)
            ),
        };
        self.set_status(text, now);
    }

    fn selected_entry_id(&self) -> Option<QueueEntryId> {
        self.player
            .queue()
            .entries()
            .get(self.queue_selected)
            .map(|e| e.id)
    }

    /// `J`/`K`: moves the selected entry; the selection follows it.
    fn move_selected_entry(&mut self, up: bool) {
        let Some(id) = self.selected_entry_id() else {
            return;
        };
        let moved = if up {
            self.player.move_up(id)
        } else {
            self.player.move_down(id)
        };
        if moved && let Some(pos) = self.player.queue().position(id) {
            self.queue_selected = pos;
        }
    }

    /// `C`: the first press asks for confirmation; a second one within
    /// [`CLEAR_CONFIRM`] (with nothing in between) clears.
    fn clear_queue(&mut self, armed: Option<Instant>, now: Instant) -> Vec<PlayerCommand> {
        if self.player.queue().is_empty() {
            return Vec::new();
        }
        if armed.is_none_or(|until| now >= until) {
            self.clear_armed_until = Some(now + CLEAR_CONFIRM);
            self.set_status(CLEAR_PROMPT, now);
            return Vec::new();
        }
        let commands = self.player.clear_queue();
        self.queue_selected = 0;
        self.set_status("Queue cleared", now);
        commands
    }

    fn select_current(&mut self, now: Instant) {
        let queue = self.player.queue();
        match queue.current().and_then(|id| queue.position(id)) {
            Some(pos) => self.queue_selected = pos,
            None => self.set_status("No current entry", now),
        }
    }

    fn shuffle_upcoming(&mut self, now: Instant) {
        let shuffled = self.player.shuffle_upcoming(&mut self.rng);
        let text = if shuffled > 1 {
            format!("Shuffled {} upcoming entries", thousands(shuffled))
        } else {
            "Nothing to shuffle".to_owned()
        };
        self.set_status(text, now);
    }

    /// `S`: asks for a name and saves the queue's Navidrome songs, in
    /// queue order, as a new playlist. Local tracks are left out.
    fn save_queue_as_playlist(&mut self, now: Instant) {
        let Some(browser) = self.browser.as_mut() else {
            self.set_status(NAVIDROME_NOT_CONFIGURED, now);
            return;
        };
        let entries = self.player.queue().entries();
        let songs: Vec<String> = entries
            .iter()
            .filter_map(|e| e.track.navidrome_id().map(str::to_owned))
            .collect();
        let local = entries.len() - songs.len();
        if songs.is_empty() {
            self.set_status(NO_REMOTE_IN_QUEUE, now);
            return;
        }
        browser.open_save_queue(songs, local, &queue_playlist_name(SystemTime::now()));
    }

    /// `D`: the first press asks for confirmation; a second one within
    /// [`CLEAR_CONFIRM`] on the same playlist deletes it.
    fn delete_playlist(&mut self, armed: Option<(EditArm, Instant)>, now: Instant) {
        let (id, name) = match self.browser_mut().deletion() {
            Ok(target) => target,
            Err("") => return,
            Err(reason) => return self.set_status(reason, now),
        };
        let arm = EditArm::Delete(id.clone());
        if confirms(armed, &arm, now) {
            let text = format!("Deleting {name}…");
            if self.browser_mut().delete_playlist(id, name) {
                self.set_status(text, now);
            }
        } else {
            self.edit_armed = Some((arm, now + CLEAR_CONFIRM));
            self.set_status(format!("Press D again to delete {name}"), now);
        }
    }

    /// `d` in a playlist: the first press asks for confirmation; a second
    /// one within [`CLEAR_CONFIRM`] on the same entry removes it.
    fn remove_from_playlist(&mut self, armed: Option<(EditArm, Instant)>, now: Instant) {
        let removal: Removal = match self.browser_mut().removal() {
            Ok(removal) => removal,
            Err("") => return,
            Err(reason) => return self.set_status(reason, now),
        };
        let arm = EditArm::Remove {
            playlist_id: removal.playlist_id.clone(),
            index: removal.index,
            song_id: removal.song_id.clone(),
        };
        if confirms(armed, &arm, now) {
            let text = format!("Removing {}…", removal.song);
            if self.browser_mut().remove_entry(removal) {
                self.set_status(text, now);
            }
        } else {
            self.edit_armed = Some((arm, now + CLEAR_CONFIRM));
            let again = format!("d again to remove {}", removal.song);
            let text = match removal.hidden {
                Hidden::None => format!("Press {again}"),
                hidden => browse::hidden_warning(hidden, &again),
            };
            self.set_status(text, now);
        }
    }

    /// `w`: saves the shown unsaved order. If the playlist has entries the
    /// server hides, which the save would drop, the first press warns and
    /// a second one within [`CLEAR_CONFIRM`] (with nothing in between)
    /// saves.
    fn save_order(&mut self, armed: Option<(EditArm, Instant)>, now: Instant) {
        let Some(playlist_id) = self
            .browser_mut()
            .unsaved_order_playlist()
            .map(str::to_owned)
        else {
            return;
        };
        let arm = EditArm::SaveOrder(playlist_id);
        let confirmed = confirms(armed, &arm, now);
        match self.browser_mut().save_order(confirmed) {
            Some(SaveOrder::Message(message)) => self.set_status(message, now),
            Some(SaveOrder::Confirm(hidden)) => {
                self.edit_armed = Some((arm, now + CLEAR_CONFIRM));
                self.set_status(browse::hidden_warning(hidden, "w again to save"), now);
            }
            None => {}
        }
    }

    fn remove_selected(&mut self) -> Vec<PlayerCommand> {
        match self.player.queue().entries().get(self.queue_selected) {
            Some(entry) => {
                let id = entry.id;
                self.player.remove(id)
            }
            None => Vec::new(),
        }
    }
}

/// `true` if `armed` is `arm`, pressed before its deadline.
fn confirms(armed: Option<(EditArm, Instant)>, arm: &EditArm, now: Instant) -> bool {
    armed.is_some_and(|(armed, until)| armed == *arm && now < until)
}

/// The suggested name of a saved queue: `Queue 2026-11-01`, the local
/// date (the UTC date if the local one cannot be determined).
pub fn queue_playlist_name(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = local_date(secs).unwrap_or_else(|| {
        let (y, m, d) = civil_from_days(secs / 86_400);
        (i64::try_from(y).unwrap_or(i64::MAX), m, d)
    });
    format!("Queue {year:04}-{month:02}-{day:02}")
}

/// The local calendar date of `secs` after the Unix epoch, by the
/// process's time zone (`TZ`, else the system's), via `localtime_r`.
fn local_date(secs: u64) -> Option<(i64, u64, u64)> {
    let time = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `tm` is plain data, fully written by `localtime_r` on
    // success; both pointers are valid for the call, and `localtime_r`
    // (unlike `localtime`) uses no shared static buffer. It may read the
    // `TZ` environment variable (to load the time zone); that is sound
    // because Coda never modifies its environment at runtime (no
    // `set_var`/`remove_var`), so no other thread writes it meanwhile.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some((
        i64::from(tm.tm_year) + 1900,
        u64::try_from(tm.tm_mon).ok()? + 1,
        u64::try_from(tm.tm_mday).ok()?,
    ))
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard
/// Hinnant's `civil_from_days`, for non-negative days).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// `1 track`, `1,234 tracks`.
pub fn count(n: usize, noun: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{} {noun}{plural}", thousands(n))
}

pub use crate::model::thousands;

// ----- terminal lifecycle and event loop -------------------------------

/// Runs the TUI until the user quits, a terminating signal arrives, or a
/// terminal error occurs. On every one of those exits the playback backend
/// is shut down (bounded by [`SHUTDOWN_TIMEOUT`]) before the terminal is
/// restored. A panic on the UI thread restores the terminal from the panic
/// hook; the backend's child is then killed as unwinding drops it.
///
/// With a music directory, a scan starts after the TUI is up. With a
/// Navidrome client, a background ping reports the connection state in the
/// status line; neither ever blocks the loop.
///
/// `restored` (already loaded) is restored before the loop starts and its
/// notice shown. With a save path, changes are saved there during the
/// session (debounced) and once more on exit, bounded by [`SAVE_TIMEOUT`].
///
/// With `scrobble` (and a client), plays of Navidrome tracks are reported
/// to the server by a background [`Scrobbler`].
///
/// `settings` is what the Settings overlay shows and may change; a save
/// still running at exit is awaited for up to [`SETTINGS_SAVE_WAIT`].
pub async fn run(
    mut playback: PlaybackHandle,
    music_dir: Option<PathBuf>,
    navidrome: Option<navidrome::Client>,
    scrobble: bool,
    restored: session::Restored,
    settings: settings::Context,
) -> io::Result<()> {
    let signals = match Signals::new() {
        Ok(signals) => signals,
        Err(error) => {
            shutdown_backend(playback).await;
            return Err(error);
        }
    };
    // Taken before `try_init`, which wraps whatever hook is installed.
    let previous_hook = std::panic::take_hook();
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            install_panic_hook(previous_hook);
            shutdown_backend(playback).await;
            restore_terminal();
            return Err(error);
        }
    };
    install_panic_hook(previous_hook);
    if let Err(error) = crossterm::execute!(io::stdout(), EnableBracketedPaste) {
        tracing::warn!(%error, "could not enable bracketed paste");
    }
    // Never dropped implicitly: if a panic unwinds out of the loop after
    // the terminal hung up, `Terminal::drop` would `eprintln!` on the dead
    // tty, panic while panicking and abort, so `kill_on_drop` would never
    // reap mpv. Leaked during unwinding (the hook already restored the
    // terminal); released explicitly on normal paths. See LESSONS.md, "Do not
    // let Ratatui's `Terminal` drop on a hung-up tty".
    // The `run` future must be awaited to completion, never cancelled
    // (e.g. by `select!` or a timeout): cancelling it would leak the
    // terminal without restoring it or shutting the backend down.
    let mut terminal = ManuallyDrop::new(terminal);

    let scan = music_dir.clone().map(scan::spawn_scan);
    let ping = navidrome
        .clone()
        .map(|client| spawn_ping(client, Generation::default()));
    let scrobbler = navidrome.clone().filter(|_| scrobble).map(Scrobbler::spawn);
    let mut app = App::with_sources(music_dir, navidrome);
    let mut writers = Writers {
        settings: SettingsIo::new(settings.config_path.clone(), SecretStore::system),
        edits: EditRunner::default(),
    };
    app.set_settings_context(settings);
    app.set_scrobbling(scrobbler.is_some());
    let now = Instant::now();
    playback.send_all(app.restore_session(restored.session, now));
    if let Some(notice) = restored.notice {
        app.set_status(notice, now);
    }
    let mut saver = Saver::new(restored.save_path);
    let (scrobbler, scrobble_failure) = scrobbler.unzip();
    let sources = Sources {
        scan,
        ping,
        scrobbler,
        scrobble_failure,
    };
    let result = event_loop(
        &mut terminal,
        &mut app,
        &mut playback,
        sources,
        signals,
        &mut saver,
        &mut writers,
    )
    .await;
    if let Err(error) = &result {
        tracing::error!(%error, "terminal error; quitting");
    }
    let queued = app.browser().map_or(0, Browser::queued_edits);
    if queued > 0 {
        tracing::info!(queued, "queued playlist changes dropped at exit");
    }
    tokio::join!(
        saver.finish(&mut app),
        shutdown_backend(playback),
        writers.settings.finish(SETTINGS_SAVE_WAIT),
        writers.edits.finish(EDIT_SAVE_WAIT)
    );
    release_terminal(ManuallyDrop::into_inner(terminal));
    restore_terminal();
    result
}

/// Writes one snapshot (blocking). [`session::save`] outside tests.
type SaveFn = Arc<dyn Fn(&Path, &Session) -> io::Result<()> + Send + Sync>;

/// Writes session snapshots on a blocking thread, one at a time, so an
/// older snapshot can never land after a newer one.
struct Saver {
    /// `None`: saving is off.
    path: Option<PathBuf>,
    write: SaveFn,
    /// The running write; `true` if it succeeded.
    in_flight: Option<JoinHandle<bool>>,
}

impl Saver {
    fn new(path: Option<PathBuf>) -> Self {
        Self::with_writer(path, Arc::new(session::save))
    }

    fn with_writer(path: Option<PathBuf>, write: SaveFn) -> Self {
        Self {
            path,
            write,
            in_flight: None,
        }
    }

    /// Collects a finished write and starts the next one if a save is due.
    /// While a write is running (or its result cannot be collected yet), a
    /// due save waits for a later call.
    fn poll(&mut self, app: &mut App, now: Instant) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(handle) = &mut self.in_flight {
            // `None` also when Tokio's coop budget is spent: keep the
            // handle and try again next time.
            match handle.now_or_never() {
                None => return,
                Some(result) => {
                    self.in_flight = None;
                    if !matches!(result, Ok(true)) {
                        app.save_failed();
                    }
                }
            }
        }
        if let Some(session) = app.take_due_save(now) {
            self.in_flight = Some(spawn_save(self.write.clone(), path.clone(), session));
        }
    }

    /// The exit save: waits for a running write, then writes the final
    /// snapshot if anything changed (or the last write failed). Bounded by
    /// [`SAVE_TIMEOUT`]; a write still running after that is abandoned
    /// (the old file stays intact).
    async fn finish(mut self, app: &mut App) {
        let Some(path) = self.path.take() else {
            return;
        };
        let in_flight = self.in_flight.take();
        let write = self.write.clone();
        let work = async {
            if let Some(handle) = in_flight
                && !matches!(handle.await, Ok(true))
            {
                app.save_failed();
            }
            if let Some(session) = app.take_final_save()
                && !matches!(spawn_save(write, path, session).await, Ok(true))
            {
                app.save_failed();
            }
        };
        if tokio::time::timeout(SAVE_TIMEOUT, work).await.is_err() {
            tracing::warn!("session save did not finish in time");
        }
    }
}

fn spawn_save(write: SaveFn, path: PathBuf, session: Session) -> JoinHandle<bool> {
    tokio::task::spawn_blocking(move || match write(&path, &session) {
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

async fn shutdown_backend(playback: PlaybackHandle) {
    let outcome = playback.shutdown(SHUTDOWN_TIMEOUT).await;
    tracing::info!(?outcome, "playback backend shut down");
}

/// Shows the cursor and drops the terminal. Ratatui's `Terminal::drop`
/// retries a failed show-cursor with `eprintln!`, which panics when the
/// terminal has hung up; in that case the terminal is leaked instead (the
/// process is exiting).
pub(crate) fn release_terminal(mut terminal: DefaultTerminal) {
    if let Err(error) = terminal.show_cursor() {
        tracing::warn!(%error, "could not show the cursor");
        std::mem::forget(terminal);
    }
}

/// Restores the terminal without ever writing to stderr: after a hangup
/// the terminal is gone and a failed write must not panic.
pub(crate) fn restore_terminal() {
    if let Err(error) = crossterm::execute!(io::stdout(), DisableBracketedPaste) {
        tracing::warn!(%error, "could not disable bracketed paste");
    }
    if let Err(error) = ratatui::try_restore() {
        tracing::warn!(%error, "could not restore the terminal");
    }
}

pub type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Replaces the hook `ratatui::try_init` installed (which restores the
/// terminal on every thread, and may write to stderr while doing so) with
/// one that records the calling (UI) thread. For panics on the UI thread it
/// restores the terminal and chains to `previous`, the hook active before
/// `try_init`. Panics on other threads (Tokio workers, the scan's
/// `catch_unwind`-guarded tag parser) are only logged, so they never touch
/// the terminal or stderr. Unwinding stays enabled.
pub fn install_panic_hook(previous: PanicHook) {
    drop(std::panic::take_hook());
    let ui_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if std::thread::current().id() == ui_thread {
            tracing::error!(%message, %location, "panic on the UI thread");
            let _ = ratatui::try_restore();
            // `try_restore` does not show the cursor, and the leaked
            // terminal (see `run`) never will. Errors are ignored so a
            // hung-up tty stays safe.
            let _ = crossterm::execute!(
                std::io::stdout(),
                DisableBracketedPaste,
                crossterm::cursor::Show
            );
            previous(info);
        } else {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("unnamed");
            tracing::error!(%message, %location, thread = name, "panic on a background thread");
        }
    }));
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".into())
}

/// Terminating signals that quit gracefully. In raw mode Ctrl-C arrives as
/// a key, so `SIGINT` only comes from outside (`kill -INT`).
struct Signals {
    terminate: Signal,
    hangup: Signal,
    interrupt: Signal,
}

impl Signals {
    fn new() -> io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }
}

type PingResult = Result<(), navidrome::Error>;

/// Background work whose results arrive in the loop.
struct Sources {
    scan: Option<mpsc::Receiver<ScanEvent>>,
    ping: Option<oneshot::Receiver<(Generation, PingResult)>>,
    /// Sends the app's scrobbles.
    scrobbler: Option<Scrobbler>,
    /// The first scrobble failure, if any.
    scrobble_failure: Option<oneshot::Receiver<navidrome::Error>>,
}

/// Runs the browser's fetches for the current client, at most
/// [`MAX_CONCURRENT_FETCHES`] at a time.
struct Fetches {
    limit: Arc<Semaphore>,
    /// Fetch tasks that may still be running.
    running: Vec<JoinHandle<()>>,
}

impl Fetches {
    fn new() -> Self {
        Self {
            limit: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            running: Vec::new(),
        }
    }

    /// Spawns the fetch tasks for `requests`; their responses arrive on
    /// `responses`.
    fn spawn(
        &mut self,
        requests: Vec<FetchRequest>,
        client: Option<&navidrome::Client>,
        responses: &mpsc::UnboundedSender<FetchResponse>,
    ) {
        let Some(client) = client else {
            return;
        };
        if requests.is_empty() {
            return;
        }
        self.running.retain(|task| !task.is_finished());
        for request in requests {
            self.running.push(browse::spawn_fetch(
                client.clone(),
                self.limit.clone(),
                request,
                responses.clone(),
            ));
        }
    }

    /// The client changed: aborts the old client's fetches (each still
    /// answers, with `Aborted`, which the generation check drops) and gives
    /// the new client its own permits, so nothing queued for the old server
    /// runs or holds up the new one.
    fn restart(&mut self) {
        for task in self.running.drain(..) {
            task.abort();
        }
        self.limit = Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES));
    }
}

/// Work that changes something outside Coda (the config file and
/// keychain, the user's playlists); each is waited for, bounded, at exit.
struct Writers {
    settings: SettingsIo,
    edits: EditRunner,
}

/// Runs playlist edits one at a time, across client changes too: the next
/// edit (of any generation) starts only once the running one has
/// answered. Edits are not aborted when the client changes (the old
/// server's change completes; its result is dropped by the generation
/// check).
#[derive(Default)]
struct EditRunner {
    /// The edit running: its generation, id and task.
    running: Option<(Generation, EditId, JoinHandle<()>)>,
}

impl EditRunner {
    /// Starts the app's next edit with its current client, unless one is
    /// running.
    fn pump(&mut self, app: &mut App, responses: &mpsc::UnboundedSender<EditResponse>) {
        if self.running.is_some() {
            return;
        }
        // A browser, and so an edit, exists only with a client.
        let Some(client) = app.navidrome_client().cloned() else {
            return;
        };
        for request in app.take_edit_requests() {
            let key = (request.generation, request.id);
            let task = navidrome::edit::spawn_edit(client.clone(), request, responses.clone());
            self.running = Some((key.0, key.1, task));
            app.edit_in_flight = true;
        }
    }

    /// An edit answered; if it is the running one, the next may start.
    fn answered(&mut self, app: &mut App, response: &EditResponse) {
        if self.running.as_ref().is_some_and(|(generation, id, _)| {
            (*generation, *id) == (response.generation, response.id)
        }) {
            self.running = None;
            app.edit_in_flight = false;
            if app.browser.as_ref().is_none_or(|b| b.pending_edits() == 0) {
                app.edit_quit_armed = false;
            }
        }
    }

    /// Waits up to `wait` for the running edit at exit.
    async fn finish(self, wait: Duration) {
        let Some((_, _, task)) = self.running else {
            return;
        };
        if task.is_finished() {
            return;
        }
        tracing::info!("waiting for a playlist change before exit");
        if tokio::time::timeout(wait, task).await.is_err() {
            tracing::warn!("a playlist change did not finish before exit");
        }
    }
}

/// Performs the Settings overlay's requests: the ping on a Tokio task, the
/// keychain and config writes on blocking threads. Each request answers
/// exactly once, through a drop guard. Logs only kinds and outcomes.
struct SettingsIo {
    config_path: Option<PathBuf>,
    open_store: fn() -> Result<SecretStore, SecretError>,
    /// Saves and disconnects that may still be running.
    writes: Vec<JoinHandle<()>>,
}

impl SettingsIo {
    fn new(
        config_path: Option<PathBuf>,
        open_store: fn() -> Result<SecretStore, SecretError>,
    ) -> Self {
        Self {
            config_path,
            open_store,
            writes: Vec::new(),
        }
    }

    fn spawn(
        &mut self,
        request: settings::Request,
        tx: &mpsc::UnboundedSender<settings::Response>,
    ) {
        use settings::{NO_CONFIG_LOCATION, Request, Response};
        self.writes.retain(|write| !write.is_finished());
        fn interrupted<T>() -> Result<T, String> {
            Err(setup::INTERRUPTED.to_owned())
        }
        let open_store = self.open_store;
        match request {
            Request::Ping { id, server } => {
                let reply = Reply::new(
                    tx,
                    Response::Pinged {
                        id,
                        result: interrupted(),
                    },
                );
                tokio::spawn(async move {
                    let result = setup::ping(server).await;
                    reply.send(Response::Pinged { id, result });
                });
            }
            Request::Save {
                id,
                server,
                previous_account,
            } => {
                let reply = Reply::new(
                    tx,
                    Response::Saved {
                        id,
                        result: interrupted(),
                    },
                );
                let Some(path) = self.config_path.clone() else {
                    reply.send(Response::Saved {
                        id,
                        result: Err(NO_CONFIG_LOCATION.into()),
                    });
                    return;
                };
                self.writes.push(tokio::task::spawn_blocking(move || {
                    let result =
                        save_server(&path, server, open_store, previous_account.as_deref());
                    reply.send(Response::Saved { id, result });
                }));
            }
            Request::Disconnect {
                id,
                previous_account,
            } => {
                let reply = Reply::new(
                    tx,
                    Response::Disconnected {
                        id,
                        result: interrupted(),
                    },
                );
                let Some(path) = self.config_path.clone() else {
                    reply.send(Response::Disconnected {
                        id,
                        result: Err(NO_CONFIG_LOCATION.into()),
                    });
                    return;
                };
                self.writes.push(tokio::task::spawn_blocking(move || {
                    let result = setup::save_navidrome(
                        &path,
                        SaveNavidrome::Remove,
                        open_store,
                        previous_account.as_deref(),
                    )
                    .map(drop)
                    .map_err(save_message);
                    reply.send(Response::Disconnected { id, result });
                }));
            }
        }
    }

    /// Waits up to `wait` for running saves, so the process does not exit
    /// between the keychain write and the config write.
    async fn finish(self, wait: Duration) {
        if self.writes.iter().all(JoinHandle::is_finished) {
            return;
        }
        let all = futures_util::future::join_all(self.writes);
        if tokio::time::timeout(wait, all).await.is_err() {
            tracing::warn!("a settings save did not finish before exit");
        }
    }
}

/// Builds the client for `server`, then stores the password and writes
/// `[navidrome]` (keeping the folder and `scrobble`). Blocking.
fn save_server(
    path: &Path,
    server: crate::onboarding::Server,
    open_store: fn() -> Result<SecretStore, SecretError>,
    previous_account: Option<&str>,
) -> Result<settings::Saved, String> {
    let client = navidrome::Client::new(
        server.url.clone(),
        server.username.clone(),
        server.password.clone(),
    )
    .map_err(|error| {
        tracing::warn!(detail = %error.detail(), "settings: could not set up the client");
        "Could not set up the Navidrome client".to_owned()
    })?;
    let scrobble = setup::save_navidrome(
        path,
        SaveNavidrome::Server(server),
        open_store,
        previous_account,
    )
    .map_err(save_message)?;
    Ok(settings::Saved { client, scrobble })
}

fn save_message(error: crate::onboarding::SaveError) -> String {
    match error {
        crate::onboarding::SaveError::Keychain(message)
        | crate::onboarding::SaveError::Config(message) => message,
    }
}

/// Pings the server on a Tokio task; the result arrives over a oneshot,
/// tagged with the client's generation.
fn spawn_ping(
    client: navidrome::Client,
    generation: Generation,
) -> oneshot::Receiver<(Generation, PingResult)> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send((generation, client.ping().await));
    });
    rx
}

/// Waits for a one-off result (the ping, the first scrobble failure);
/// pends forever once it has been taken. `None` if the sender was dropped.
async fn next_once<T>(once: &mut Option<oneshot::Receiver<T>>) -> Option<T> {
    match once {
        Some(rx) => rx.await.ok(),
        None => std::future::pending().await,
    }
}

/// Waits for the next scan event; pends forever once the scan channel has
/// been closed and dropped, so the branch never spins.
async fn next_scan_event(scan: &mut Option<mpsc::Receiver<ScanEvent>>) -> Option<ScanEvent> {
    match scan {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    playback: &mut PlaybackHandle,
    sources: Sources,
    mut signals: Signals,
    saver: &mut Saver,
    writers: &mut Writers,
) -> io::Result<()> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut view = render::ViewState::default();
    let Sources {
        mut scan,
        mut ping,
        mut scrobbler,
        mut scrobble_failure,
    } = sources;
    let mut fetches = Fetches::new();
    let (fetch_tx, mut fetch_rx) = mpsc::unbounded_channel();
    let (settings_tx, mut settings_rx) = mpsc::unbounded_channel();
    let (edit_tx, mut edit_rx) = mpsc::unbounded_channel();
    let mut playback_open = true;
    let mut dirty = true;
    // Only the visualizer changed: redraw just its area over a copy of the
    // last full frame (`last_frame`, kept only while it is shown).
    let mut visualizer_dirty = false;
    let mut last_frame: Option<Buffer> = None;

    loop {
        for request in app.take_settings_requests() {
            writers.settings.spawn(request, &settings_tx);
        }
        if let Some(change) = app.take_client_change() {
            fetches.restart();
            // Replacing the receivers drops the old ping's answer and the
            // old scrobbler, whose worker ends once its queue is sent.
            ping = change
                .client
                .clone()
                .map(|client| spawn_ping(client, change.generation));
            (scrobbler, scrobble_failure) = change
                .client
                .filter(|_| change.scrobble)
                .map(Scrobbler::spawn)
                .unzip();
        }
        fetches.spawn(app.take_fetch_requests(), app.navidrome_client(), &fetch_tx);
        writers.edits.pump(app, &edit_tx);
        for request in app.take_scrobble_requests() {
            if let Some(scrobbler) = &scrobbler {
                scrobbler.send(request);
            }
        }
        if dirty {
            let completed =
                terminal.draw(|frame| render::draw(frame, app, &mut view, Instant::now()))?;
            last_frame = view.visualizer_area.map(|_| completed.buffer.clone());
            visualizer_dirty = false;
            if let Some(command) = app.set_visualizer_drawable(view.visualizer_drawable) {
                playback.send(command);
            }
            app.clamp_help_scroll(view.help_max_scroll);
            app.set_remote_rows(view.remote_rows);
            // A larger screen may want the next page of a list at once;
            // draw again right away so "Loading more…" shows.
            let requests = app.take_fetch_requests();
            dirty = !requests.is_empty();
            fetches.spawn(requests, app.navidrome_client(), &fetch_tx);
            if dirty {
                continue;
            }
        } else if visualizer_dirty {
            visualizer_dirty = false;
            if let (Some(area), Some(frame)) = (view.visualizer_area, last_frame.as_mut()) {
                render::redraw_visualizer(terminal, frame, app, area)?;
            }
        }
        tokio::select! {
            input_event = input.next() => match input_event {
                Some(Ok(Event::Key(key))) if app.settings_open() => {
                    if key.kind != KeyEventKind::Release
                        && let Some(input) = crate::ui::onboarding::map_key(key)
                    {
                        app.handle_settings_input(input, Instant::now());
                        dirty = true;
                    }
                }
                Some(Ok(Event::Key(key))) => {
                    let action = if app.input_open() {
                        keymap::map_input_key(key)
                    } else {
                        keymap::map_key(key, app.focus(), app.help_open())
                    };
                    if key.kind != KeyEventKind::Release
                        && let Some(action) = action
                    {
                        let commands = app.handle_action(action, Instant::now());
                        playback.send_all(commands);
                        dirty = true;
                    }
                }
                Some(Ok(Event::Paste(text))) => {
                    if app.input_open() || app.settings_open() {
                        app.handle_paste(&text);
                        dirty = true;
                    }
                }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input closed",
                    ));
                }
            },
            event = playback.recv(), if playback_open => match event {
                Some(PlaybackEvent::Visualizer { load, frame }) => {
                    if app.handle_visualizer(load, &frame, Instant::now()) {
                        visualizer_dirty = true;
                    }
                }
                Some(event) => {
                    let commands = app.handle_playback_event(event, Instant::now());
                    playback.send_all(commands);
                    dirty = true;
                }
                // After the (possibly synthetic) BackendExited: stop polling.
                None => playback_open = false,
            },
            event = next_scan_event(&mut scan) => {
                match event {
                    Some(event) => app.handle_scan_event(event, Instant::now()),
                    None => {
                        scan = None;
                        app.scan_closed(Instant::now());
                    }
                }
                dirty = true;
            },
            // The loop holds a sender, so this never yields `None`.
            Some(response) = fetch_rx.recv() => {
                app.handle_fetch_response(response, Instant::now());
                dirty = true;
            },
            // Likewise.
            Some(response) = edit_rx.recv() => {
                writers.edits.answered(app, &response);
                app.handle_edit_response(response, Instant::now());
                dirty = true;
            },
            // Likewise.
            Some(response) = settings_rx.recv() => {
                app.handle_settings_response(response, Instant::now());
                dirty = true;
            },
            failure = next_once(&mut scrobble_failure) => {
                scrobble_failure = None;
                // `None`: the worker ended without a failure (at exit).
                if let Some(error) = failure {
                    app.handle_scrobble_failure(error, Instant::now());
                    dirty = true;
                }
            },
            result = next_once(&mut ping) => {
                ping = None;
                // A dropped sender means the task panicked; it is logged.
                if let Some((generation, result)) = result {
                    app.handle_ping(generation, result, Instant::now());
                    dirty = true;
                }
            },
            _ = signals.terminate.recv() => {
                tracing::info!("SIGTERM received; quitting");
                return Ok(());
            },
            _ = signals.hangup.recv() => {
                tracing::info!("SIGHUP received; quitting");
                return Ok(());
            },
            _ = signals.interrupt.recv() => {
                tracing::info!("SIGINT received; quitting");
                return Ok(());
            },
            _ = tick.tick() => {
                let now = Instant::now();
                app.tick(now);
                saver.poll(app, now);
                dirty = true;
            },
        }
        if app.should_quit() {
            tracing::info!("quit requested");
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_input::MAX_QUERY_CHARS;
    use crate::model::TrackMetadata;
    use crate::playback::LoadId;
    use crate::playback::{EndReason, PlaybackTarget};

    const ROOT: &str = "/music";

    fn track(rel: &str) -> Track {
        Track::local(format!("{ROOT}/{rel}").into(), TrackMetadata::default())
    }

    fn numbered(rel: &str, no: u32) -> Track {
        Track::local(
            format!("{ROOT}/{rel}").into(),
            TrackMetadata {
                track_no: Some(no),
                ..TrackMetadata::default()
            },
        )
    }

    fn remote(id: &str) -> Track {
        Track::navidrome(
            id.into(),
            TrackMetadata {
                title: Some(format!("Remote {id}")),
                ..TrackMetadata::default()
            },
        )
    }

    fn client() -> navidrome::Client {
        navidrome::Client::new(
            navidrome::client::parse_base_url("https://h.example/nd").unwrap(),
            "u".into(),
            navidrome::Password::new("p".into()),
        )
        .unwrap()
    }

    fn stream_id(commands: &[PlaybackCommand]) -> String {
        match commands {
            [
                PlaybackCommand::Load {
                    target: PlaybackTarget::Stream(url),
                    ..
                },
            ] => {
                let url = url::Url::parse(url.expose()).unwrap();
                assert_eq!(url.path(), "/nd/rest/stream");
                url.query_pairs()
                    .find(|(k, _)| k == "id")
                    .map(|(_, v)| v.into_owned())
                    .unwrap()
            }
            other => panic!("expected a single stream Load, got {other:?}"),
        }
    }

    #[test]
    fn remote_sources_resolve_to_a_stream_url_for_the_song() {
        let mut app = App::with_sources(None, Some(client()));
        let now = Instant::now();
        app.player.enqueue(remote("song 1"));
        app.player.enqueue(remote("song-2"));
        app.focus = Focus::Queue;
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_id(&commands), "song 1");
        let load = match &commands[..] {
            [PlaybackCommand::Load { load, .. }] => *load,
            _ => unreachable!(),
        };
        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        assert_eq!(stream_id(&commands), "song-2");
    }

    #[test]
    fn remote_track_without_navidrome_fails_and_the_queue_advances() {
        let (mut app, now) = app_with(&["a.flac"]);
        app.player.enqueue(remote("r1"));
        app.player.enqueue(track("b.flac"));
        app.player.enqueue(remote("r2"));
        app.player.enqueue(remote("r3"));
        app.player.enqueue(track("c.flac"));
        app.focus = Focus::Queue;
        // A single remote entry fails; the local one after it is loaded.
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(load_path(&commands), PathBuf::from("/music/b.flac"));
        let current = app.player.queue().current_entry().unwrap();
        assert_eq!(current.track.title, "b");
        assert_eq!(
            app.status(now),
            Some("Could not play Remote r1: Navidrome is not configured")
        );
        assert_eq!(app.player.status(), crate::player::PlaybackStatus::Loading);
        // Two in a row: the remote run is skipped in one pass to the next
        // local entry.
        let load = load_id(&commands);
        app.handle_playback_event(PlaybackEvent::Started { load }, now);
        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        assert_eq!(load_path(&commands), PathBuf::from("/music/c.flac"));
        assert_eq!(
            app.status(now),
            Some("Navidrome is not configured; skipped 1 remote track")
        );
        assert_eq!(app.player.status(), crate::player::PlaybackStatus::Loading);
    }

    #[test]
    fn unresolvable_last_entry_stops_the_backend() {
        let (mut app, now) = app_with(&[]);
        app.player.enqueue(remote("r1"));
        app.focus = Focus::Queue;
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(commands, [PlaybackCommand::Stop]);
        assert_eq!(app.player.status(), crate::player::PlaybackStatus::Stopped);
        assert!(
            app.status(now)
                .unwrap()
                .contains("Navidrome is not configured")
        );
    }

    /// An app with `rels` scanned, showing Folders.
    fn app_with(rels: &[&str]) -> (App, Instant) {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        app.handle_scan_event(
            ScanEvent::Batch(rels.iter().map(|r| track(r)).collect()),
            now,
        );
        open_local(&mut app, local::HomeEntry::Folders, now);
        (app, now)
    }

    /// Goes back to the local Home and opens `entry`.
    fn open_local(app: &mut App, entry: local::HomeEntry, now: Instant) {
        while *app.local_browser().current() != local::View::Home {
            app.handle_action(Action::Parent, now);
        }
        let at = local::HOME_ENTRIES
            .iter()
            .position(|e| *e == entry)
            .unwrap();
        press(app, &[Action::Up; 4], now);
        press(app, &vec![Action::Down; at], now);
        press(app, &[Action::Activate], now);
    }

    fn row_names(app: &App) -> Vec<String> {
        app.library_rows()
            .iter()
            .map(|row| match row {
                LocalRow::Home(e) => e.label().to_owned(),
                LocalRow::Dir(d) => format!("{}/", d.name()),
                LocalRow::Track(_, t) => t.title.clone(),
                LocalRow::Artist(a) => a.name().to_owned(),
                LocalRow::Album(album, artist) => format!("{} — {}", album.name(), artist.name()),
                LocalRow::Playlist(p) => p.name.clone(),
            })
            .collect()
    }

    fn selected_name(app: &App) -> String {
        row_names(app)[app.library_selected()].clone()
    }

    fn load_path(commands: &[PlaybackCommand]) -> PathBuf {
        match commands {
            [
                PlaybackCommand::Load {
                    target: PlaybackTarget::File(path),
                    ..
                },
            ] => path.clone(),
            other => panic!("expected one Load, got {other:?}"),
        }
    }

    fn load_id(commands: &[PlaybackCommand]) -> LoadId {
        match commands {
            [PlaybackCommand::Load { load, .. }] => *load,
            other => panic!("expected one Load, got {other:?}"),
        }
    }

    fn queue_titles(app: &App) -> Vec<String> {
        let q = app.player().queue();
        q.entries()
            .iter()
            .map(|e| {
                let mark = if q.current() == Some(e.id) { "*" } else { "" };
                format!("{}{mark}", e.track.title)
            })
            .collect()
    }

    fn press(app: &mut App, actions: &[Action], now: Instant) -> Vec<PlaybackCommand> {
        actions
            .iter()
            .flat_map(|a| app.handle_action(*a, now))
            .collect()
    }

    #[test]
    fn navigates_into_and_out_of_directories() {
        let (mut app, now) = app_with(&["a/x/1.flac", "b/2.flac", "top.mp3"]);
        assert_eq!(row_names(&app), ["a/", "b/", "top"]);
        press(&mut app, &[Action::Down], now);
        assert_eq!(selected_name(&app), "b/");
        press(&mut app, &[Action::Activate], now);
        assert_eq!(app.library_path(), ["b"]);
        assert_eq!(row_names(&app), ["2"]);
        // Parent re-selects the directory we came from.
        press(&mut app, &[Action::Parent], now);
        assert!(app.library_path().is_empty());
        assert_eq!(selected_name(&app), "b/");
        // Parent at the folder root goes to Home, on Folders.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(*app.local_browser().current(), local::View::Home);
        assert_eq!(selected_name(&app), "Folders");
        // Parent at Home is a no-op.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(selected_name(&app), "Folders");
        // Nested.
        press(
            &mut app,
            &[Action::Activate, Action::Activate, Action::Activate],
            now,
        );
        assert_eq!(app.library_path(), ["a", "x"]);
        press(&mut app, &[Action::Parent], now);
        assert_eq!(app.library_path(), ["a"]);
        assert_eq!(selected_name(&app), "x/");
    }

    #[test]
    fn selection_moves_within_bounds() {
        let (mut app, now) = app_with(&["1.flac", "2.flac"]);
        press(&mut app, &[Action::Up], now);
        assert_eq!(app.library_selected(), 0);
        press(&mut app, &[Action::Down, Action::Down, Action::Down], now);
        assert_eq!(app.library_selected(), 1);
        // Empty queue: selection stays at 0.
        press(
            &mut app,
            &[Action::SwitchFocus, Action::Down, Action::Up],
            now,
        );
        assert_eq!(app.queue_selected(), 0);
    }

    #[test]
    fn selection_follows_its_item_as_batches_arrive() {
        let (mut app, now) = app_with(&["m/1.flac", "z/1.flac"]);
        press(&mut app, &[Action::Down], now);
        assert_eq!(selected_name(&app), "z/");
        // Rows are inserted before the selection; it stays on "z/".
        app.handle_scan_event(
            ScanEvent::Batch(vec![track("a/1.flac"), track("b/1.flac"), track("0.flac")]),
            now,
        );
        assert_eq!(row_names(&app), ["a/", "b/", "m/", "z/", "0"]);
        assert_eq!(selected_name(&app), "z/");

        // Inside a directory, tracks inserted before the selected track.
        press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(app.library_path(), ["m"]);
        press(&mut app, &[Action::Down], now);
        assert_eq!(app.library_selected(), 0, "only one row");
        app.handle_scan_event(ScanEvent::Batch(vec![track("m/0.flac")]), now);
        assert_eq!(row_names(&app), ["0", "1"]);
        assert_eq!(selected_name(&app), "1");
    }

    #[test]
    fn selection_is_valid_on_an_empty_library() {
        let now = Instant::now();
        for entry in local::HOME_ENTRIES {
            let mut app = App::new(ROOT.into());
            // Home is there before anything is scanned.
            assert_eq!(
                row_names(&app),
                ["Playlists", "Folders", "Artists", "Albums"]
            );
            open_local(&mut app, entry, now);
            assert!(app.library_rows().is_empty(), "{entry:?}");
            let commands = press(
                &mut app,
                &[
                    Action::Down,
                    Action::Activate,
                    Action::Enqueue,
                    Action::EnqueueNext,
                    Action::CycleSort,
                ],
                now,
            );
            assert!(commands.is_empty());
            assert_eq!(app.library_selected(), 0);
            assert!(app.player().queue().is_empty());
        }
        let mut app = App::new(ROOT.into());
        open_local(&mut app, local::HomeEntry::Folders, now);
        app.handle_scan_event(ScanEvent::Batch(vec![track("a.flac")]), now);
        assert_eq!(selected_name(&app), "a");
    }

    #[test]
    fn enter_on_a_track_inserts_after_current_and_plays() {
        let (mut app, now) = app_with(&["1.flac", "2.flac", "3.flac"]);
        let first = press(&mut app, &[Action::Activate], now);
        assert_eq!(load_path(&first), PathBuf::from("/music/1.flac"));
        assert_eq!(queue_titles(&app), ["1*"]);
        let third = press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(load_path(&third), PathBuf::from("/music/3.flac"));
        assert_eq!(queue_titles(&app), ["1", "3*"]);
        // Back on 1 and Enter on 2: inserted after current (1), played.
        app.handle_action(Action::SwitchFocus, now);
        let back = press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(load_path(&back), PathBuf::from("/music/1.flac"));
        app.handle_action(Action::SwitchFocus, now);
        let second = press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(load_path(&second), PathBuf::from("/music/2.flac"));
        assert_eq!(queue_titles(&app), ["1", "2*", "3"]);
    }

    #[test]
    fn enter_on_a_queue_entry_plays_it() {
        let (mut app, now) = app_with(&["1.flac", "2.flac"]);
        press(
            &mut app,
            &[Action::Enqueue, Action::Down, Action::Enqueue],
            now,
        );
        assert_eq!(queue_titles(&app), ["1", "2"]);
        let commands = press(
            &mut app,
            &[Action::SwitchFocus, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(load_path(&commands), PathBuf::from("/music/2.flac"));
        assert_eq!(queue_titles(&app), ["1", "2*"]);
    }

    #[test]
    fn enqueue_on_a_directory_adds_all_tracks_recursively_in_browse_order() {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        app.handle_scan_event(
            ScanEvent::Batch(vec![
                numbered("art/b-side.flac", 2),
                numbered("art/CD2/x.flac", 1),
                numbered("art/a-side.flac", 1),
                numbered("art/CD1/y.flac", 2),
                numbered("art/CD1/w.flac", 1),
                track("other/o.flac"),
            ]),
            now,
        );
        // Home entries add nothing.
        press(&mut app, &[Action::Enqueue, Action::EnqueueNext], now);
        assert!(app.player().queue().is_empty());
        open_local(&mut app, local::HomeEntry::Folders, now);
        let commands = press(&mut app, &[Action::Enqueue], now);
        assert!(commands.is_empty(), "enqueue does not start playback");
        assert_eq!(queue_titles(&app), ["w", "y", "x", "a-side", "b-side"]);
        assert_eq!(app.status(now), Some("Added 5 tracks"));
        // A single track.
        press(
            &mut app,
            &[Action::Down, Action::Activate, Action::Enqueue],
            now,
        );
        assert_eq!(queue_titles(&app).last().unwrap(), "o");
        assert_eq!(app.status(now), Some("Added o"));
    }

    #[test]
    fn panel_specific_actions_do_nothing_in_the_other_panel() {
        let (mut app, now) = app_with(&["d/1.flac"]);
        // Remove in the library: nothing.
        assert!(press(&mut app, &[Action::Remove], now).is_empty());
        app.handle_action(Action::SwitchFocus, now);
        // Enqueue / Parent in the queue: nothing.
        press(&mut app, &[Action::Enqueue, Action::Parent], now);
        assert!(app.player().queue().is_empty());
    }

    #[test]
    fn remove_maps_to_player_remove_and_clamps_selection() {
        let (mut app, now) = app_with(&["1.flac", "2.flac", "3.flac"]);
        press(
            &mut app,
            &[
                Action::Enqueue,
                Action::Down,
                Action::Enqueue,
                Action::Down,
                Action::Enqueue,
            ],
            now,
        );
        // Remove the last (not playing): no commands, selection clamps.
        app.handle_action(Action::SwitchFocus, now);
        let commands = press(&mut app, &[Action::Down, Action::Down, Action::Remove], now);
        assert!(commands.is_empty());
        assert_eq!(queue_titles(&app), ["1", "2"]);
        assert_eq!(app.queue_selected(), 1);

        // Remove the playing entry: acts like `n` (plays the next one).
        let playing = press(&mut app, &[Action::Up, Action::Activate], now);
        let load = load_id(&playing);
        app.handle_playback_event(PlaybackEvent::Started { load }, now);
        let next = press(&mut app, &[Action::Remove], now);
        assert_eq!(load_path(&next), PathBuf::from("/music/2.flac"));
        assert_eq!(queue_titles(&app), ["2*"]);
        // Remove the last playing entry: stops.
        assert_eq!(
            press(&mut app, &[Action::Remove], now),
            [PlaybackCommand::Stop]
        );
        assert!(app.player().queue().is_empty());
        assert_eq!(app.queue_selected(), 0);
        assert!(press(&mut app, &[Action::Remove], now).is_empty());
    }

    #[test]
    fn transport_actions_map_to_player_intents() {
        let (mut app, now) = app_with(&["1.flac", "2.flac"]);
        press(
            &mut app,
            &[Action::Enqueue, Action::Down, Action::Enqueue],
            now,
        );
        // Space with no current entry plays the first.
        let commands = press(&mut app, &[Action::TogglePause], now);
        let load = load_id(&commands);
        assert_eq!(load_path(&commands), PathBuf::from("/music/1.flac"));
        // Seeks are no-ops until Started.
        assert!(press(&mut app, &[Action::SeekForward], now).is_empty());
        app.handle_playback_event(PlaybackEvent::Started { load }, now);
        assert_eq!(
            press(&mut app, &[Action::SeekForward, Action::SeekBackward], now),
            [
                PlaybackCommand::SeekRelative(SEEK_STEP),
                PlaybackCommand::SeekRelative(-SEEK_STEP)
            ]
        );
        assert_eq!(
            press(&mut app, &[Action::VolumeDown, Action::VolumeUp], now),
            [
                PlaybackCommand::SetVolume(95),
                PlaybackCommand::SetVolume(100)
            ]
        );
        assert_eq!(
            press(&mut app, &[Action::TogglePause], now),
            [PlaybackCommand::SetPause(true)]
        );
        let next = press(&mut app, &[Action::Next], now);
        assert_eq!(load_path(&next), PathBuf::from("/music/2.flac"));
        assert!(press(&mut app, &[Action::Next], now).is_empty());
        assert_eq!(app.status(now), Some("End of queue"));
        let prev = press(&mut app, &[Action::Previous], now);
        assert_eq!(load_path(&prev), PathBuf::from("/music/1.flac"));
    }

    #[test]
    fn help_overlay_swallows_actions() {
        let (mut app, now) = app_with(&["1.flac"]);
        press(&mut app, &[Action::ToggleHelp], now);
        assert!(app.help_open());
        let commands = press(
            &mut app,
            &[
                Action::Activate,
                Action::Enqueue,
                Action::Down,
                Action::SwitchFocus,
            ],
            now,
        );
        assert!(commands.is_empty());
        assert!(app.player().queue().is_empty());
        assert_eq!(app.focus(), Focus::Library);
        press(&mut app, &[Action::CloseHelp], now);
        assert!(!app.help_open());
        press(&mut app, &[Action::ToggleHelp, Action::ToggleHelp], now);
        assert!(!app.help_open());
        press(&mut app, &[Action::ToggleHelp, Action::Quit], now);
        assert!(app.should_quit());
    }

    #[test]
    fn status_messages_expire_after_the_ttl() {
        let (mut app, start) = app_with(&[]);
        app.handle_action(Action::Next, start);
        assert_eq!(app.status(start), Some("Nothing is playing"));
        let almost = start + STATUS_TTL - Duration::from_millis(1);
        assert!(!app.tick(almost));
        assert_eq!(app.status(almost), Some("Nothing is playing"));
        let later = start + STATUS_TTL;
        assert_eq!(app.status(later), None);
        assert!(app.tick(later), "expiry is a visible change");
        assert!(!app.tick(later));
        // A newer message restarts the timer.
        app.set_status("one", start);
        app.set_status("two", almost);
        assert_eq!(app.status(later), Some("two"));
    }

    #[test]
    fn backend_exit_is_persistent_while_its_message_expires() {
        let (mut app, now) = app_with(&["1.flac"]);
        app.handle_playback_event(PlaybackEvent::BackendExited("mpv exited".into()), now);
        let later = now + STATUS_TTL * 3;
        app.tick(later);
        assert_eq!(app.status(later), None);
        assert_eq!(app.player().unavailable_reason(), Some("mpv exited"));
        // Playback actions are refused with a message.
        assert!(press(&mut app, &[Action::Activate], later).is_empty());
        assert!(app.status(later).unwrap().contains("unavailable"));
    }

    #[test]
    fn scan_progress_and_completion() {
        let mut app = App::new(ROOT.into());
        let now = Instant::now();
        assert_eq!(app.scan_state(), ScanState::Scanning { tracks: 0 });
        app.handle_scan_event(
            ScanEvent::Batch(vec![track("a.flac"), track("b.flac")]),
            now,
        );
        assert_eq!(app.scan_state(), ScanState::Scanning { tracks: 2 });
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 2,
                skipped: 1,
                errors: 0,
            },
            now,
        );
        assert!(matches!(
            app.scan_state(),
            ScanState::Finished { tracks: 2, .. }
        ));
        assert_eq!(
            app.status(now),
            Some("Library scan complete: 2 tracks, 1 skipped")
        );
        // Closing after Finished is normal.
        app.scan_closed(now);
        assert!(matches!(app.scan_state(), ScanState::Finished { .. }));
    }

    #[test]
    fn scan_channel_closing_without_finished_is_a_failure() {
        let (mut app, now) = app_with(&["a.flac"]);
        app.scan_closed(now);
        assert_eq!(app.scan_state(), ScanState::Failed { tracks: 1 });
        assert!(app.status(now).unwrap().contains("scan failed"));
    }

    #[test]
    fn number_formatting() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(count(1, "track"), "1 track");
        assert_eq!(count(5000, "track"), "5,000 tracks");
    }

    // ----- Navidrome browsing ------------------------------------------

    use crate::navidrome::browse::{Content, Load, Resource};
    use crate::navidrome::{Album, Artist, SearchResults};

    fn artist(id: &str) -> Artist {
        Artist {
            id: id.into(),
            name: format!("Artist {id}"),
            album_count: Some(2),
        }
    }

    fn album(id: &str) -> Album {
        Album {
            id: id.into(),
            name: format!("Album {id}"),
            artist: Some("Someone".into()),
            year: Some(1999),
            song_count: None,
            duration: None,
        }
    }

    fn songs(ids: &[&str]) -> Content {
        Content::Songs(ids.iter().map(|id| remote(id)).collect())
    }

    /// Navidrome only: the Navidrome source is active on Home; Artists is
    /// opened from it, so the artists are requested.
    fn nav_app() -> (App, Instant) {
        let (mut app, now) = nav_home();
        press(&mut app, &[Action::Down, Action::Activate], now);
        (app, now)
    }

    /// Navidrome only, on Home.
    fn nav_home() -> (App, Instant) {
        (App::with_sources(None, Some(client())), Instant::now())
    }

    fn requests(app: &mut App) -> Vec<FetchRequest> {
        app.take_fetch_requests()
    }

    fn one_request(app: &mut App) -> FetchRequest {
        let mut requests = requests(app);
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests.remove(0)
    }

    fn respond(app: &mut App, request: &FetchRequest, result: Result<Content, navidrome::Error>) {
        app.handle_fetch_response(
            FetchResponse {
                generation: request.generation,
                id: request.id,
                resource: request.resource.clone(),
                result,
            },
            Instant::now(),
        );
    }

    /// Answers the startup Artists request with `ids`.
    fn nav_app_with_artists(ids: &[&str]) -> (App, Instant) {
        let (mut app, now) = nav_app();
        let request = one_request(&mut app);
        assert_eq!(request.resource, Resource::Artists);
        respond(
            &mut app,
            &request,
            Ok(Content::Artists(ids.iter().map(|id| artist(id)).collect())),
        );
        (app, now)
    }

    fn browser(app: &App) -> &Browser {
        app.browser().unwrap()
    }

    /// The titles of the views below Home (which is always the root).
    fn view_titles(app: &App) -> Vec<String> {
        let stack = browser(app).stack();
        assert_eq!(stack[0].resource, Resource::Home);
        stack[1..].iter().map(|v| v.title.clone()).collect()
    }

    fn current_load(app: &App) -> Load {
        browser(app).current().unwrap().load.clone()
    }

    fn current_selected(app: &App) -> usize {
        browser(app).current().unwrap().selected
    }

    #[test]
    fn navidrome_only_starts_on_home_and_fetches_nothing_until_opened() {
        let (mut app, now) = nav_home();
        assert_eq!(app.source(), Source::Navidrome);
        assert_eq!(app.connection(), &Connection::Connecting);
        assert_eq!(current_load(&app), Load::Ready);
        assert!(view_titles(&app).is_empty());
        assert!(requests(&mut app).is_empty(), "Home is static");
        // `h` and `a` on Home do nothing; `r` fetches nothing.
        press(
            &mut app,
            &[Action::Parent, Action::Enqueue, Action::Refresh],
            now,
        );
        assert!(view_titles(&app).is_empty());
        assert!(requests(&mut app).is_empty());
        assert!(app.player().queue().is_empty());
        press(&mut app, &[Action::Down, Action::Activate], now);
        assert!(matches!(current_load(&app), Load::Loading(_)));
        assert_eq!(view_titles(&app), ["Artists"]);
        assert_eq!(one_request(&mut app).resource, Resource::Artists);
        assert!(requests(&mut app).is_empty(), "taken once");
        // With a local library too, Local is active and nothing is fetched.
        let mut both = App::with_sources(Some(ROOT.into()), Some(client()));
        assert_eq!(both.source(), Source::Local);
        assert!(requests(&mut both).is_empty());
    }

    #[test]
    fn navigation_stack_opens_artists_and_albums_and_plays_songs() {
        let (mut app, now) = nav_app_with_artists(&["a1", "a2"]);
        assert_eq!(current_load(&app), Load::Ready);
        assert_eq!(app.connection(), &Connection::Connected);
        press(&mut app, &[Action::Down, Action::Activate], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a2"]);
        let request = one_request(&mut app);
        assert_eq!(request.resource, Resource::Artist("a2".into()));
        respond(
            &mut app,
            &request,
            Ok(Content::Albums(vec![album("x"), album("y")])),
        );
        press(&mut app, &[Action::Down, Action::Activate], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a2", "Album y"]);
        let request = one_request(&mut app);
        assert_eq!(request.resource, Resource::Album("y".into()));
        respond(&mut app, &request, Ok(songs(&["s1", "s2", "s3"])));

        // Enter on a song plays it now, inserted after the current entry.
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_id(&commands), "s1");
        let commands = press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(stream_id(&commands), "s3");
        assert_eq!(queue_titles(&app), ["Remote s1", "Remote s3*"]);
        app.focus = Focus::Queue;
        press(&mut app, &[Action::Activate], now); // back to s1
        app.focus = Focus::Library;
        let commands = press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(stream_id(&commands), "s2");
        assert_eq!(queue_titles(&app), ["Remote s1", "Remote s2*", "Remote s3"]);
        assert_eq!(app.status(now), Some("Playing Remote s2"));

        // Up the stack: each view keeps its selection; Home stays.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a2"]);
        assert_eq!(current_selected(&app), 1);
        press(&mut app, &[Action::Parent], now);
        assert_eq!(view_titles(&app), ["Artists"]);
        assert_eq!(current_selected(&app), 1);
        press(&mut app, &[Action::Parent, Action::Parent], now);
        assert!(view_titles(&app).is_empty());
        assert_eq!(current_selected(&app), 1, "Home keeps its selection");
        // Re-opening a cached list needs no request.
        press(&mut app, &[Action::Activate], now);
        assert_eq!(view_titles(&app), ["Artists"]);
        assert_eq!(current_load(&app), Load::Ready);
        assert!(requests(&mut app).is_empty());
    }

    #[test]
    fn stale_responses_are_cached_but_do_not_change_the_view() {
        let (mut app, now) = nav_app_with_artists(&["a1", "a2"]);
        press(&mut app, &[Action::Activate], now);
        let first = one_request(&mut app);
        // Leave before it answers and open another artist.
        press(
            &mut app,
            &[Action::Parent, Action::Down, Action::Activate],
            now,
        );
        let second = one_request(&mut app);
        assert_eq!(second.resource, Resource::Artist("a2".into()));
        respond(&mut app, &first, Ok(Content::Albums(vec![album("x")])));
        assert_eq!(view_titles(&app), ["Artists", "Artist a2"]);
        assert_eq!(current_load(&app), Load::Loading(second.id));
        assert!(browser(&app).cached(&first.resource).is_some());
        // Going back to the first artist shows the cached albums at once.
        press(
            &mut app,
            &[Action::Parent, Action::Up, Action::Activate],
            now,
        );
        assert_eq!(current_load(&app), Load::Ready);
        assert!(requests(&mut app).is_empty());
        // The second response still lands in the cache.
        respond(&mut app, &second, Ok(Content::Albums(vec![album("z")])));
        assert_eq!(current_load(&app), Load::Ready);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
        assert!(browser(&app).cached(&second.resource).is_some());
    }

    #[test]
    fn errors_show_inline_and_refresh_retries() {
        let (mut app, now) = nav_app();
        let request = one_request(&mut app);
        respond(&mut app, &request, Err(navidrome::Error::Auth));
        assert_eq!(
            current_load(&app),
            Load::Failed("authentication failed".into())
        );
        assert_eq!(
            app.connection(),
            &Connection::Connecting,
            "fetch errors keep the ping state"
        );
        // Nothing to act on while failed.
        assert!(
            press(
                &mut app,
                &[Action::Down, Action::Activate, Action::Enqueue],
                now
            )
            .is_empty()
        );
        press(&mut app, &[Action::Refresh], now);
        let retry = one_request(&mut app);
        assert_eq!(retry.resource, Resource::Artists);
        assert_ne!(retry.id, request.id);
        assert_eq!(current_load(&app), Load::Loading(retry.id));
        // Refresh while loading awaits the same request.
        press(&mut app, &[Action::Refresh], now);
        assert!(requests(&mut app).is_empty());
        respond(&mut app, &retry, Ok(Content::Artists(vec![artist("a")])));
        assert_eq!(current_load(&app), Load::Ready);
    }

    #[test]
    fn refresh_refetches_and_replaces_the_cache_entry() {
        let (mut app, now) = nav_app_with_artists(&["a1", "a2", "a3"]);
        press(&mut app, &[Action::Down, Action::Down], now);
        press(&mut app, &[Action::Refresh], now);
        let request = one_request(&mut app);
        assert!(browser(&app).cached(&Resource::Artists).is_none());
        assert!(matches!(current_load(&app), Load::Loading(_)));
        respond(
            &mut app,
            &request,
            Ok(Content::Artists(vec![artist("b1"), artist("b2")])),
        );
        let Some(Content::Artists(artists)) = browser(&app).cached(&Resource::Artists) else {
            panic!("artists not cached");
        };
        assert_eq!(artists[0].id, "b1");
        // The selection is kept, clamped to the new list.
        assert_eq!(current_selected(&app), 1);
        // `r` in the local source does nothing.
        press(&mut app, &[Action::SelectLocal, Action::Refresh], now);
        assert!(requests(&mut app).is_empty());
    }

    #[test]
    fn album_and_artist_enqueue_after_fetch_in_press_order() {
        let (mut app, now) = nav_app_with_artists(&["a1", "a2"]);
        // `a` on artist a1, then on artist a2: both need their albums.
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(app.status(now), Some("Fetching Artist a1…"));
        press(&mut app, &[Action::Down, Action::Enqueue], now);
        let mut reqs = requests(&mut app);
        assert_eq!(
            reqs.iter().map(|r| r.resource.clone()).collect::<Vec<_>>(),
            [Resource::Artist("a1".into()), Resource::Artist("a2".into())]
        );
        let (a2, a1) = (reqs.pop().unwrap(), reqs.pop().unwrap());
        // a2 answers first, entirely: it waits for a1.
        respond(&mut app, &a2, Ok(Content::Albums(vec![album("z")])));
        let z = one_request(&mut app);
        respond(&mut app, &z, Ok(songs(&["z1"])));
        assert!(app.player().queue().is_empty());
        // a1's albums arrive; its two albums answer in reverse order.
        respond(
            &mut app,
            &a1,
            Ok(Content::Albums(vec![album("x"), album("y")])),
        );
        let mut reqs = requests(&mut app);
        assert_eq!(reqs.len(), 2);
        let (y, x) = (reqs.pop().unwrap(), reqs.pop().unwrap());
        assert_eq!(x.resource, Resource::Album("x".into()));
        respond(&mut app, &y, Ok(songs(&["y1", "y2"])));
        assert!(app.player().queue().is_empty());
        respond(&mut app, &x, Ok(songs(&["x1", "x2"])));
        assert_eq!(
            queue_titles(&app),
            [
                "Remote x1",
                "Remote x2",
                "Remote y1",
                "Remote y2",
                "Remote z1"
            ]
        );
        // The last completed job reports; a single track is named.
        assert_eq!(app.status(now), Some("Added Remote z1"));
        assert!(app.player().status() == crate::player::PlaybackStatus::Stopped);
        assert_eq!(browser(&app).pending_jobs(), 0);
        // Cached now: an album enqueues at once.
        press(&mut app, &[Action::Activate], now);
        press(&mut app, &[Action::Enqueue], now);
        assert!(requests(&mut app).is_empty());
        assert_eq!(app.player().queue().len(), 6);
        assert_eq!(app.status(now), Some("Added Remote z1"));
    }

    #[test]
    fn song_enqueue_waits_behind_an_earlier_album_and_failures_are_dropped_in_turn() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Activate], now);
        let albums = one_request(&mut app);
        respond(
            &mut app,
            &albums,
            Ok(Content::Albums(vec![album("x"), album("y")])),
        );
        // `a` on album x (not fetched), then open album y and `a` a song.
        press(&mut app, &[Action::Enqueue], now);
        let x = one_request(&mut app);
        press(&mut app, &[Action::Down, Action::Activate], now);
        let y = one_request(&mut app);
        respond(&mut app, &y, Ok(songs(&["y1", "y2"])));
        press(&mut app, &[Action::Down, Action::Enqueue], now);
        assert!(app.player().queue().is_empty(), "waits for album x");
        assert_eq!(app.status(now), Some("Queued Remote y2 after Album x…"));
        respond(&mut app, &x, Err(navidrome::Error::Http(500)));
        assert_eq!(queue_titles(&app), ["Remote y2"]);
        // The failure is not overwritten by the song added in the same batch.
        assert_eq!(
            app.status(now),
            Some("Could not add Album x: server returned HTTP 500; added 1 track")
        );
        assert_eq!(browser(&app).pending_jobs(), 0);
        // A later job for the failed album fetches it again.
        press(
            &mut app,
            &[Action::Parent, Action::Up, Action::Enqueue],
            now,
        );
        assert_eq!(one_request(&mut app).resource, Resource::Album("x".into()));
    }

    // ----- play next (`A`) ------------------------------------------------

    #[test]
    fn local_play_next_on_a_folder_and_tracks_keeps_press_order() {
        let (mut app, now) = app_with(&["a.flac", "b.flac", "d/1.flac", "d/2.flac"]);
        assert_eq!(row_names(&app), ["d/", "a", "b"]);
        // Play a, append b.
        press(
            &mut app,
            &[
                Action::Down,
                Action::Activate,
                Action::Down,
                Action::Enqueue,
            ],
            now,
        );
        assert_eq!(queue_titles(&app), ["a*", "b"]);
        // `A` on the folder, then on b: both right after a, in that order.
        let commands = press(
            &mut app,
            &[Action::Up, Action::Up, Action::EnqueueNext],
            now,
        );
        assert!(commands.is_empty(), "play next never starts playback");
        assert_eq!(app.status(now), Some("Playing next: 2 tracks"));
        press(
            &mut app,
            &[Action::Down, Action::Down, Action::EnqueueNext],
            now,
        );
        assert_eq!(app.status(now), Some("Playing next: b"));
        assert_eq!(queue_titles(&app), ["a*", "1", "2", "b", "b"]);
        // Playing another track starts a new run.
        press(&mut app, &[Action::Up, Action::Activate], now);
        press(&mut app, &[Action::Down, Action::EnqueueNext], now);
        assert_eq!(queue_titles(&app), ["a", "a*", "b", "1", "2", "b", "b"]);
    }

    // ----- local Home, tag lists and playlists ---------------------------

    /// A local track with tags: `artist` and `album_artist` may be empty
    /// (absent).
    fn tagged(rel: &str, title: &str, artist: &str, album_artist: &str, album: &str) -> Track {
        let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        Track::local(
            format!("{ROOT}/{rel}").into(),
            TrackMetadata {
                title: some(title),
                artist: some(artist),
                album_artist: some(album_artist),
                album: some(album),
                track_no: rel
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.get(..2))
                    .and_then(|n| n.parse().ok()),
                ..TrackMetadata::default()
            },
        )
    }

    /// Two artists, a compilation and an untagged file.
    fn tag_library() -> Vec<Track> {
        vec![
            tagged("m/x/02.flac", "M Two", "Miles", "", "Kind"),
            tagged("m/x/01.flac", "M One", "miles", "", "kind"),
            tagged("m/y/01.flac", "Y One", "Miles", "", "Blue"),
            tagged("c/01.flac", "C One", "Guest", "Various", "Hits"),
            tagged("c/02.flac", "C Two", "Miles", "Various", "Hits"),
            tagged("loose.flac", "", "", "", ""),
        ]
    }

    fn tag_app() -> (App, Instant) {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        app.handle_scan_event(ScanEvent::Batch(tag_library()), now);
        (app, now)
    }

    fn path_titles(app: &App) -> Vec<String> {
        app.local_browser().path_titles(app.library())
    }

    fn local_playlist(name: &str, rels: &[&str], unresolved: usize) -> ScanEvent {
        ScanEvent::Playlist(crate::library::m3u::PlaylistFile {
            path: format!("{ROOT}/{name}.m3u").into(),
            name: name.into(),
            entries: rels.iter().map(|r| format!("{ROOT}/{r}").into()).collect(),
            unresolved,
            cut: false,
        })
    }

    #[test]
    fn local_home_opens_each_list_and_h_goes_back_up_to_home() {
        let (mut app, now) = tag_app();
        assert_eq!(
            row_names(&app),
            ["Playlists", "Folders", "Artists", "Albums"]
        );
        assert!(path_titles(&app).is_empty());
        open_local(&mut app, local::HomeEntry::Artists, now);
        // "Miles" is the more common spelling.
        assert_eq!(row_names(&app), ["Miles", "Various", "Unknown artist"]);
        assert_eq!(path_titles(&app), ["Artists"]);
        press(&mut app, &[Action::Activate], now);
        assert_eq!(row_names(&app), ["Blue — Miles", "Kind — Miles"]);
        press(&mut app, &[Action::Down, Action::Activate], now);
        assert_eq!(row_names(&app), ["M One", "M Two"]);
        assert_eq!(path_titles(&app), ["Artists", "Miles", "Kind"]);
        // `h` back up: each level re-selects what it opened, ending at Home.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(selected_name(&app), "Kind — Miles");
        press(&mut app, &[Action::Parent], now);
        assert_eq!(selected_name(&app), "Miles");
        press(&mut app, &[Action::Parent], now);
        assert_eq!(selected_name(&app), "Artists");
        press(&mut app, &[Action::Parent, Action::Parent], now);
        assert_eq!(*app.local_browser().current(), local::View::Home);
        // Albums: every album with its artist, A–Z by name.
        open_local(&mut app, local::HomeEntry::Albums, now);
        assert_eq!(path_titles(&app), ["Albums (A–Z)"]);
        assert_eq!(
            row_names(&app),
            [
                "Blue — Miles",
                "Hits — Various",
                "Kind — Miles",
                "Unknown album — Unknown artist"
            ]
        );
        // Search from a list returns to it.
        local_search_for(&mut app, "one", now);
        assert_eq!(local_titles(&app), ["C One", "M One", "Y One"]);
        press(&mut app, &[Action::Parent], now);
        assert_eq!(path_titles(&app), ["Albums (A–Z)"]);
        // Folders is the tree, unchanged.
        open_local(&mut app, local::HomeEntry::Folders, now);
        assert_eq!(row_names(&app), ["c/", "m/", "loose"]);
        assert_eq!(path_titles(&app), ["Folders"]);
        press(
            &mut app,
            &[Action::Down, Action::Activate, Action::Activate],
            now,
        );
        assert_eq!(path_titles(&app), ["Folders", "m", "x"]);
    }

    #[test]
    fn o_cycles_the_local_album_order_and_keeps_the_selected_album() {
        let (mut app, now) = tag_app();
        // Outside the Albums list, `o` does nothing.
        press(&mut app, &[Action::CycleSort], now);
        assert_eq!(*app.local_browser().current(), local::View::Home);
        open_local(&mut app, local::HomeEntry::Albums, now);
        press(&mut app, &[Action::Down], now);
        assert_eq!(selected_name(&app), "Hits — Various");
        press(&mut app, &[Action::CycleSort], now);
        assert_eq!(path_titles(&app), ["Albums (by artist)"]);
        assert_eq!(
            row_names(&app),
            [
                "Blue — Miles",
                "Kind — Miles",
                "Hits — Various",
                "Unknown album — Unknown artist"
            ]
        );
        assert_eq!(selected_name(&app), "Hits — Various");
        // Home → Albums reopens the order last chosen.
        open_local(&mut app, local::HomeEntry::Albums, now);
        assert_eq!(path_titles(&app), ["Albums (by artist)"]);
        press(&mut app, &[Action::CycleSort], now);
        assert_eq!(path_titles(&app), ["Albums (A–Z)"]);
    }

    #[test]
    fn local_lists_fill_in_during_a_scan_and_keep_the_selection() {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        open_local(&mut app, local::HomeEntry::Artists, now);
        assert!(app.library_rows().is_empty());
        app.handle_scan_event(
            ScanEvent::Batch(vec![tagged("1.flac", "a", "Moby", "", "Play")]),
            now,
        );
        assert_eq!(row_names(&app), ["Moby"]);
        app.handle_scan_event(
            ScanEvent::Batch(vec![
                tagged("2.flac", "b", "ABBA", "", "Gold"),
                tagged("3.flac", "c", "Zappa", "", "Apostrophe"),
            ]),
            now,
        );
        assert_eq!(row_names(&app), ["ABBA", "Moby", "Zappa"]);
        assert_eq!(selected_name(&app), "Moby");
        // Inside an album, a track arriving before the selection.
        press(&mut app, &[Action::Activate, Action::Activate], now);
        assert_eq!(row_names(&app), ["a"]);
        app.handle_scan_event(
            ScanEvent::Batch(vec![tagged("0.flac", "0", "Moby", "", "Play")]),
            now,
        );
        assert_eq!(row_names(&app), ["0", "a"]);
        assert_eq!(selected_name(&app), "a");
        // Playlists arrive after the tracks.
        open_local(&mut app, local::HomeEntry::Playlists, now);
        assert!(app.library_rows().is_empty());
        app.handle_scan_event(local_playlist("Road", &["1.flac"], 0), now);
        app.handle_scan_event(local_playlist("Alpha", &[], 0), now);
        assert_eq!(row_names(&app), ["Alpha", "Road"]);
        assert_eq!(selected_name(&app), "Road");
        app.handle_scan_event(local_playlist("Morning", &[], 0), now);
        assert_eq!(row_names(&app), ["Alpha", "Morning", "Road"]);
        assert_eq!(selected_name(&app), "Road");
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 4,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        assert_eq!(
            app.status(now),
            Some("Library scan complete: 4 tracks, 3 playlists")
        );
    }

    #[test]
    fn local_artist_album_and_playlist_enqueue_in_order() {
        let (mut app, now) = tag_app();
        app.handle_scan_event(
            local_playlist(
                "Mix",
                &["c/02.flac", "m/x/01.flac", "gone.flac", "c/02.flac"],
                2,
            ),
            now,
        );
        // `a` on an artist: albums A–Z, each in track order.
        open_local(&mut app, local::HomeEntry::Artists, now);
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(queue_titles(&app), ["Y One", "M One", "M Two"]);
        assert_eq!(app.status(now), Some("Added 3 tracks"));
        // Play the first, then `A` an album and a playlist: both next, in
        // press order, the playlist with its duplicate.
        press(
            &mut app,
            &[Action::SwitchFocus, Action::Activate, Action::SwitchFocus],
            now,
        );
        open_local(&mut app, local::HomeEntry::Albums, now);
        press(&mut app, &[Action::Down, Action::EnqueueNext], now);
        assert_eq!(app.status(now), Some("Playing next: 2 tracks"));
        open_local(&mut app, local::HomeEntry::Playlists, now);
        assert_eq!(row_names(&app), ["Mix"]);
        press(&mut app, &[Action::EnqueueNext], now);
        assert_eq!(
            queue_titles(&app),
            [
                "Y One*", "C One", "C Two", "C Two", "M One", "C Two", "M One", "M Two"
            ]
        );
        // Inside the playlist: matched tracks in file order, duplicates
        // kept; Enter plays one.
        press(&mut app, &[Action::Activate], now);
        assert_eq!(path_titles(&app), ["Playlists", "Mix"]);
        assert_eq!(row_names(&app), ["C Two", "M One", "C Two"]);
        let commands = press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(load_path(&commands), PathBuf::from("/music/c/02.flac"));
        // The selection stays on the second "C Two" as batches arrive.
        app.handle_scan_event(ScanEvent::Batch(vec![track("z.flac")]), now);
        assert_eq!(app.library_selected(), 2);
        // `a` on an album row and on a track in an album.
        open_local(&mut app, local::HomeEntry::Albums, now);
        press(&mut app, &[Action::Up; 4], now);
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(app.status(now), Some("Added Miles - Y One"));
        press(&mut app, &[Action::Activate, Action::Enqueue], now);
        assert_eq!(app.status(now), Some("Added Miles - Y One"));
    }

    #[test]
    fn an_empty_playlist_adds_nothing() {
        let (mut app, now) = tag_app();
        app.handle_scan_event(local_playlist("Gone", &["nope.flac"], 1), now);
        open_local(&mut app, local::HomeEntry::Playlists, now);
        press(&mut app, &[Action::Enqueue], now);
        assert!(app.player().queue().is_empty());
        assert_eq!(app.status(now), Some("Nothing to add"));
        press(&mut app, &[Action::Activate], now);
        assert!(app.library_rows().is_empty());
        assert!(press(&mut app, &[Action::Activate, Action::Enqueue], now).is_empty());
    }

    #[test]
    fn without_a_local_library_local_keys_do_nothing() {
        let now = Instant::now();
        let mut app = App::with_sources(None, Some(client()));
        press(&mut app, &[Action::SelectLocal], now);
        let commands = press(
            &mut app,
            &[
                Action::Down,
                Action::Activate,
                Action::Activate,
                Action::Enqueue,
                Action::CycleSort,
            ],
            now,
        );
        assert!(commands.is_empty());
        assert_eq!(*app.local_browser().current(), local::View::Home);
        assert_eq!(app.library_selected(), 0);
    }

    #[test]
    fn play_next_with_nothing_current_appends() {
        let (mut app, now) = app_with(&["a.flac", "b.flac"]);
        press(
            &mut app,
            &[Action::EnqueueNext, Action::Down, Action::EnqueueNext],
            now,
        );
        assert_eq!(queue_titles(&app), ["a", "b"]);
        // Local search results too.
        local_search_for(&mut app, "a", now);
        press(&mut app, &[Action::EnqueueNext], now);
        assert_eq!(queue_titles(&app), ["a", "b", "a"]);
        assert_eq!(app.status(now), Some("Added a"), "nothing plays next");
    }

    #[test]
    fn remote_play_next_jobs_keep_their_turn() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Activate], now);
        let albums = one_request(&mut app);
        respond(
            &mut app,
            &albums,
            Ok(Content::Albums(vec![album("x"), album("y")])),
        );
        press(&mut app, &[Action::Down, Action::Activate], now);
        let y = one_request(&mut app);
        respond(&mut app, &y, Ok(songs(&["y1", "y2"])));
        press(&mut app, &[Action::Activate], now);
        assert_eq!(queue_titles(&app), ["Remote y1*"]);
        // `A` on album x (not fetched yet), then on song y2 (ready).
        press(
            &mut app,
            &[Action::Parent, Action::Up, Action::EnqueueNext],
            now,
        );
        let x = one_request(&mut app);
        assert_eq!(app.status(now), Some("Fetching Album x…"));
        press(
            &mut app,
            &[
                Action::Down,
                Action::Activate,
                Action::Down,
                Action::EnqueueNext,
            ],
            now,
        );
        assert_eq!(app.status(now), Some("Queued Remote y2 after Album x…"));
        assert_eq!(queue_titles(&app), ["Remote y1*"]);
        respond(&mut app, &x, Ok(songs(&["x1", "x2"])));
        assert_eq!(
            queue_titles(&app),
            ["Remote y1*", "Remote x1", "Remote x2", "Remote y2"]
        );
        assert_eq!(app.status(now), Some("Playing next: Remote y2"));
    }

    #[test]
    fn a_pending_play_next_job_keeps_press_order_after_an_unrelated_remove() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Activate], now);
        let albums = one_request(&mut app);
        respond(
            &mut app,
            &albums,
            Ok(Content::Albums(vec![album("x"), album("y")])),
        );
        press(&mut app, &[Action::Down, Action::Activate], now);
        let y = one_request(&mut app);
        respond(&mut app, &y, Ok(songs(&["y1", "y2", "y3"])));
        // Play y1, append y3, `A` y2: y1* y2 y3.
        press(&mut app, &[Action::Activate], now);
        press(
            &mut app,
            &[Action::Down, Action::Down, Action::Enqueue],
            now,
        );
        press(&mut app, &[Action::Up, Action::EnqueueNext], now);
        assert_eq!(queue_titles(&app), ["Remote y1*", "Remote y2", "Remote y3"]);
        // `A` on album x (still to fetch), then remove y3 meanwhile.
        press(
            &mut app,
            &[Action::Parent, Action::Up, Action::EnqueueNext],
            now,
        );
        let x = one_request(&mut app);
        app.focus = Focus::Queue;
        press(&mut app, &[Action::Down, Action::Down, Action::Remove], now);
        app.focus = Focus::Library;
        respond(&mut app, &x, Ok(songs(&["x1", "x2"])));
        assert_eq!(
            queue_titles(&app),
            ["Remote y1*", "Remote y2", "Remote x1", "Remote x2"]
        );
    }

    #[test]
    fn o_cycles_the_album_sort_only_in_an_album_list() {
        let (mut app, now) = nav_home();
        press(&mut app, &[Action::CycleSort], now);
        assert!(requests(&mut app).is_empty(), "nothing on Home");
        press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(view_titles(&app), ["Albums (A–Z)"]);
        let first = one_request(&mut app);
        assert_eq!(
            first.resource,
            Resource::AlbumPage {
                sort: navidrome::AlbumSort::Name,
                offset: 0
            }
        );
        press(&mut app, &[Action::CycleSort], now);
        assert_eq!(view_titles(&app), ["Albums (by artist)"]);
        let next = one_request(&mut app);
        assert_eq!(
            next.resource,
            Resource::AlbumPage {
                sort: navidrome::AlbumSort::Artist,
                offset: 0
            }
        );
        // The first order still lands in its cache.
        respond(
            &mut app,
            &first,
            Ok(Content::AlbumPage(navidrome::AlbumPage {
                albums: vec![album("a")],
                served: 1,
            })),
        );
        assert!(
            browser(&app)
                .cached(&Resource::Albums(navidrome::AlbumSort::Name))
                .is_some()
        );
        // `o` in the local source does nothing.
        press(&mut app, &[Action::SelectLocal, Action::CycleSort], now);
        assert!(requests(&mut app).is_empty());
    }

    #[test]
    fn failed_job_reports_its_error() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Enqueue], now);
        let request = one_request(&mut app);
        respond(&mut app, &request, Err(navidrome::Error::Http(500)));
        assert_eq!(
            app.status(now),
            Some("Could not add Artist a1: server returned HTTP 500")
        );
        assert!(app.player().queue().is_empty());
    }

    fn type_text(app: &mut App, text: &str, now: Instant) {
        for c in text.chars() {
            app.handle_action(Action::InputChar(c), now);
        }
    }

    fn input(app: &App) -> Option<&str> {
        browser(app).input()
    }

    #[test]
    fn search_input_edits_and_cancels() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::OpenSearch], now);
        assert!(app.input_open());
        assert_eq!(input(&app), Some(""));
        type_text(&mut app, "ab\u{7}c\u{1b}", now);
        assert_eq!(input(&app), Some("abc"), "control characters are ignored");
        press(&mut app, &[Action::InputBackspace], now);
        assert_eq!(input(&app), Some("ab"));
        // Every other action is swallowed while typing, except Quit.
        let commands = press(
            &mut app,
            &[
                Action::Next,
                Action::Enqueue,
                Action::SelectLocal,
                Action::ToggleHelp,
            ],
            now,
        );
        assert!(commands.is_empty());
        assert_eq!(app.source(), Source::Navidrome);
        assert!(!app.help_open());
        assert!(app.player().queue().is_empty());
        // Capped at 256 chars.
        type_text(&mut app, &"雨".repeat(300), now);
        assert_eq!(
            input(&app).unwrap().chars().count(),
            browse::MAX_QUERY_CHARS
        );
        press(&mut app, &[Action::InputCancel], now);
        assert!(!app.input_open());
        assert!(requests(&mut app).is_empty());
        assert_eq!(view_titles(&app), ["Artists"]);
        // A blank query only closes the input.
        press(&mut app, &[Action::OpenSearch], now);
        type_text(&mut app, "   ", now);
        press(&mut app, &[Action::InputSubmit], now);
        assert!(!app.input_open());
        assert!(requests(&mut app).is_empty());
        // Quit works while typing.
        press(&mut app, &[Action::OpenSearch, Action::Quit], now);
        assert!(app.should_quit());
    }

    #[test]
    fn search_results_open_and_enqueue_like_browsing() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::OpenSearch], now);
        type_text(&mut app, " blue ", now);
        press(&mut app, &[Action::InputSubmit], now);
        assert!(!app.input_open());
        let request = one_request(&mut app);
        assert_eq!(request.resource, Resource::Search("blue".into()));
        assert_eq!(view_titles(&app), ["Artists", "Search \"blue\""]);
        respond(
            &mut app,
            &request,
            Ok(Content::Search(SearchResults {
                artists: vec![artist("ar")],
                albums: vec![album("al")],
                tracks: vec![remote("so")],
            })),
        );
        // Rows: header, artist, header, album, header, song; headers are
        // skipped by the selection.
        assert_eq!(current_selected(&app), 1);
        press(&mut app, &[Action::Up], now);
        assert_eq!(current_selected(&app), 1);
        press(&mut app, &[Action::Down], now);
        assert_eq!(current_selected(&app), 3);
        press(&mut app, &[Action::Down, Action::Down], now);
        assert_eq!(current_selected(&app), 5);
        // Songs: `a` adds, Enter plays.
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(queue_titles(&app), ["Remote so"]);
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_id(&commands), "so");
        // Albums open.
        press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(
            view_titles(&app),
            ["Artists", "Search \"blue\"", "Album al"]
        );
        assert_eq!(one_request(&mut app).resource, Resource::Album("al".into()));
        // `h` back to the results, then to the view before the search.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(current_selected(&app), 3);
        press(&mut app, &[Action::Parent], now);
        assert_eq!(view_titles(&app), ["Artists"]);
    }

    #[test]
    fn source_switching_preserves_each_position() {
        let now = Instant::now();
        let mut app = App::with_sources(Some(ROOT.into()), Some(client()));
        app.handle_scan_event(
            ScanEvent::Batch(vec![track("d/1.flac"), track("d/2.flac")]),
            now,
        );
        open_local(&mut app, local::HomeEntry::Folders, now);
        press(&mut app, &[Action::Activate, Action::Down], now);
        assert_eq!(app.library_path(), ["d"]);
        assert_eq!(selected_name(&app), "2");
        // `2` from the queue panel focuses the library on Navidrome.
        press(
            &mut app,
            &[Action::SwitchFocus, Action::SelectNavidrome],
            now,
        );
        assert_eq!(app.focus(), Focus::Library);
        assert_eq!(app.source(), Source::Navidrome);
        assert!(requests(&mut app).is_empty(), "Home fetches nothing");
        press(&mut app, &[Action::Down, Action::Activate], now);
        let request = one_request(&mut app);
        respond(
            &mut app,
            &request,
            Ok(Content::Artists(vec![
                artist("a"),
                artist("b"),
                artist("c"),
            ])),
        );
        press(&mut app, &[Action::Down, Action::Down], now);
        press(&mut app, &[Action::SwitchFocus, Action::SelectLocal], now);
        assert_eq!(app.focus(), Focus::Library);
        assert_eq!(app.source(), Source::Local);
        assert_eq!(app.library_path(), ["d"]);
        assert_eq!(selected_name(&app), "2");
        // Local keys act on the local tree.
        press(&mut app, &[Action::Up], now);
        assert_eq!(selected_name(&app), "1");
        press(&mut app, &[Action::SelectNavidrome], now);
        assert_eq!(current_selected(&app), 2);
        assert!(requests(&mut app).is_empty(), "started only once");
        // `/` on the local source opens the local search.
        press(&mut app, &[Action::SelectLocal, Action::OpenSearch], now);
        assert!(app.input_open());
        assert_eq!(app.local_input(), Some(""));
        assert_eq!(browser(&app).input(), None);
    }

    #[test]
    fn navidrome_without_navidrome_offers_to_connect() {
        let (mut app, now) = app_with(&["1.flac", "2.flac"]);
        app.set_settings_context(settings_context(
            Some(ROOT),
            settings::Navidrome::NotConfigured,
        ));
        press(
            &mut app,
            &[Action::SwitchFocus, Action::SelectNavidrome],
            now,
        );
        assert_eq!(app.source(), Source::Navidrome);
        assert_eq!(app.focus(), Focus::Library);
        assert!(app.navidrome_prompt());
        assert_eq!(app.status(now), None);
        assert!(app.browser().is_none());
        // Library keys have nothing to act on; no local search opens.
        press(
            &mut app,
            &[
                Action::Refresh,
                Action::OpenSearch,
                Action::Enqueue,
                Action::Down,
                Action::Parent,
            ],
            now,
        );
        assert!(!app.input_open());
        assert!(app.player().queue().is_empty());
        assert!(requests(&mut app).is_empty());
        // The hidden local selection did not move.
        assert_eq!(selected_name(&app), "1");
        press(
            &mut app,
            &[Action::Down, Action::Down, Action::Up, Action::Down],
            now,
        );
        assert_eq!(app.library_selected(), 0);
        // A response without a browser is ignored.
        app.handle_fetch_response(
            FetchResponse {
                generation: Generation::default(),
                id: browse::RequestId(1),
                resource: Resource::Artists,
                result: Ok(Content::Artists(Vec::new())),
            },
            now,
        );
        // Enter opens the Connect form.
        press(&mut app, &[Action::Activate], now);
        assert!(app.settings_open());
        let (kind, _) = app.settings().form().unwrap();
        assert_eq!(kind, settings::FormKind::Connect);
        // `1` still shows the local library.
        app.handle_settings_input(Input::Esc, now);
        app.handle_settings_input(Input::Esc, now);
        assert!(!app.settings_open());
        press(&mut app, &[Action::SelectLocal], now);
        assert!(!app.navidrome_prompt());
        assert_eq!(selected_name(&app), "1");
    }

    #[test]
    fn ping_sets_the_connection_state() {
        let (mut app, now) = nav_app();
        app.handle_navidrome_ping(Err(navidrome::Error::Auth), now);
        assert_eq!(
            app.connection(),
            &Connection::Failed("authentication failed".into())
        );
        app.handle_navidrome_ping(Ok(()), now);
        assert_eq!(app.connection(), &Connection::Connected);
    }

    #[test]
    fn esc_leaves_navidrome_search_results_only() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Activate], now);
        // Not a search view: Esc does nothing.
        press(&mut app, &[Action::Back], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
        press(&mut app, &[Action::OpenSearch], now);
        type_text(&mut app, "blue", now);
        press(&mut app, &[Action::InputSubmit], now);
        assert_eq!(
            view_titles(&app),
            ["Artists", "Artist a1", "Search \"blue\""]
        );
        // In the queue panel, Esc is not a library key.
        press(
            &mut app,
            &[Action::SwitchFocus, Action::Back, Action::SwitchFocus],
            now,
        );
        assert_eq!(
            view_titles(&app),
            ["Artists", "Artist a1", "Search \"blue\""]
        );
        press(&mut app, &[Action::Back], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
        press(&mut app, &[Action::Back], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
    }

    #[test]
    fn a_search_from_search_results_replaces_them() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::Activate], now);
        for query in ["blue", "red", "green"] {
            press(&mut app, &[Action::OpenSearch], now);
            type_text(&mut app, query, now);
            press(&mut app, &[Action::InputSubmit], now);
        }
        assert_eq!(
            view_titles(&app),
            ["Artists", "Artist a1", "Search \"green\""]
        );
        press(&mut app, &[Action::Parent], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
    }

    #[test]
    fn paste_goes_into_the_search_input_only_and_never_submits() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        // Closed input: ignored.
        app.handle_paste("ignored\n");
        assert!(!app.input_open());
        press(&mut app, &[Action::OpenSearch], now);
        type_text(&mut app, "x", now);
        app.handle_paste("blue\nmoon\r\njazz\tbop\u{1b}\r");
        assert!(app.input_open(), "a pasted newline does not submit");
        assert_eq!(input(&app), Some("xblue moon jazz bop "));
        assert!(requests(&mut app).is_empty());
        // A huge paste stops at the cap.
        app.handle_paste(&"z".repeat(1_000_000));
        assert_eq!(
            input(&app).unwrap().chars().count(),
            browse::MAX_QUERY_CHARS
        );
        assert!(input(&app).unwrap().starts_with("xblue moon jazz bop "));
    }

    // ----- local search ------------------------------------------------------

    fn local_titles(app: &App) -> Vec<String> {
        app.local_search()
            .map(|s| s.results.tracks.iter().map(|t| t.title.clone()).collect())
            .unwrap_or_default()
    }

    fn local_search_for(app: &mut App, query: &str, now: Instant) {
        press(app, &[Action::OpenSearch], now);
        type_text(app, query, now);
        press(app, &[Action::InputSubmit], now);
    }

    #[test]
    fn local_search_plays_enqueues_and_returns_to_the_browse_position() {
        let (mut app, now) = app_with(&[
            "blues/a.flac",
            "blues/b.flac",
            "rock/blue moon.flac",
            "x.flac",
        ]);
        app.player.enqueue(track("x.flac"));
        app.focus = Focus::Queue;
        press(&mut app, &[Action::Activate], now);
        app.focus = Focus::Library;
        press(&mut app, &[Action::Down, Action::Activate], now);
        assert_eq!(app.library_path(), ["rock"]);
        assert_eq!(selected_name(&app), "blue moon");

        local_search_for(&mut app, "BLUE", now);
        assert!(!app.input_open());
        assert_eq!(local_titles(&app), ["a", "b", "blue moon"]);
        // `app_with` never finishes its scan, so the search is partial.
        let search = app.local_search().unwrap();
        assert_eq!(
            (search.results.total, search.partial, search.selected),
            (3, true, 0)
        );
        // Selection stays within the results.
        press(
            &mut app,
            &[Action::Up, Action::Down, Action::Down, Action::Down],
            now,
        );
        assert_eq!(app.local_search().unwrap().selected, 2);
        press(&mut app, &[Action::Up], now);
        // `a` enqueues the selected result.
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(queue_titles(&app), ["x*", "b"]);
        assert_eq!(app.status(now), Some("Added b"));
        // Enter plays it now, after the current entry.
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(load_path(&commands), PathBuf::from("/music/blues/b.flac"));
        assert_eq!(queue_titles(&app), ["x", "b*", "b"]);
        // `h` returns to where browsing left off.
        press(&mut app, &[Action::Parent], now);
        assert!(app.local_search().is_none());
        assert_eq!(app.library_path(), ["rock"]);
        assert_eq!(selected_name(&app), "blue moon");
        // So does Esc (`Back`); Backspace maps to `Parent` like `h`.
        local_search_for(&mut app, "x", now);
        assert_eq!(local_titles(&app), ["x"]);
        press(&mut app, &[Action::Back], now);
        assert!(app.local_search().is_none());
        assert_eq!(selected_name(&app), "blue moon");
        // `Back` outside results changes nothing.
        press(&mut app, &[Action::Back], now);
        assert_eq!(app.library_path(), ["rock"]);
    }

    #[test]
    fn local_search_input_edits_cancels_and_ignores_blank_queries() {
        let (mut app, now) = app_with(&["blue.flac", "red.flac"]);
        press(&mut app, &[Action::OpenSearch], now);
        assert!(app.input_open());
        type_text(&mut app, "re\u{7}x\u{1b}", now);
        assert_eq!(app.local_input(), Some("rex"));
        press(&mut app, &[Action::InputBackspace], now);
        assert_eq!(app.local_input(), Some("re"));
        // Other actions are swallowed while typing.
        let commands = press(
            &mut app,
            &[
                Action::Activate,
                Action::Enqueue,
                Action::ToggleHelp,
                Action::SwitchFocus,
            ],
            now,
        );
        assert!(commands.is_empty());
        assert!(app.player().queue().is_empty());
        assert!(!app.help_open());
        assert_eq!(app.focus(), Focus::Library);
        // Esc cancels without searching.
        press(&mut app, &[Action::InputCancel], now);
        assert!(!app.input_open());
        assert!(app.local_search().is_none());
        // Results stay when a later search is cancelled or blank.
        local_search_for(&mut app, "red", now);
        assert_eq!(local_titles(&app), ["red"]);
        press(&mut app, &[Action::OpenSearch, Action::InputCancel], now);
        local_search_for(&mut app, "   ", now);
        assert!(!app.input_open());
        assert_eq!(local_titles(&app), ["red"]);
        // A new search replaces the results.
        local_search_for(&mut app, "blue", now);
        assert_eq!(local_titles(&app), ["blue"]);
        // Capped like the Navidrome input; pastes go to the local input.
        press(&mut app, &[Action::OpenSearch], now);
        app.handle_paste("a\nb");
        assert_eq!(app.local_input(), Some("a b"));
        app.handle_paste(&"z".repeat(10_000));
        assert_eq!(app.local_input().unwrap().chars().count(), MAX_QUERY_CHARS);
        // Quit works while typing.
        press(&mut app, &[Action::Quit], now);
        assert!(app.should_quit());
    }

    #[test]
    fn local_search_during_a_scan_searches_what_was_scanned() {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        app.handle_scan_event(ScanEvent::Batch(vec![track("blue 1.flac")]), now);
        local_search_for(&mut app, "blue", now);
        let search = app.local_search().unwrap();
        assert!(search.partial);
        assert_eq!(search.results.total, 1);
        // Later batches do not change the shown results.
        app.handle_scan_event(ScanEvent::Batch(vec![track("blue 2.flac")]), now);
        assert_eq!(local_titles(&app), ["blue 1"]);
        // A scan that ended early is still partial.
        let mut failed = App::new(ROOT.into());
        failed.handle_scan_event(ScanEvent::Batch(vec![track("blue.flac")]), now);
        failed.scan_closed(now);
        local_search_for(&mut failed, "blue", now);
        assert!(failed.local_search().unwrap().partial);
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 2,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        local_search_for(&mut app, "blue", now);
        let search = app.local_search().unwrap();
        assert!(!search.partial);
        assert_eq!(local_titles(&app), ["blue 1", "blue 2"]);
        // An empty result list is harmless for every results key.
        local_search_for(&mut app, "nothing", now);
        let commands = press(
            &mut app,
            &[Action::Down, Action::Activate, Action::Enqueue],
            now,
        );
        assert!(commands.is_empty());
        assert!(app.player().queue().is_empty());
    }

    #[test]
    fn local_results_survive_source_switches_and_need_a_library() {
        let now = Instant::now();
        let mut app = App::with_sources(Some(ROOT.into()), Some(client()));
        app.handle_scan_event(ScanEvent::Batch(vec![track("blue.flac")]), now);
        local_search_for(&mut app, "blue", now);
        press(&mut app, &[Action::SelectNavidrome], now);
        // Navidrome keys act on Navidrome while the local results wait.
        press(&mut app, &[Action::Parent, Action::Back], now);
        press(&mut app, &[Action::SelectLocal], now);
        assert_eq!(local_titles(&app), ["blue"]);
        // In the queue panel, library keys do not touch the results.
        press(&mut app, &[Action::SwitchFocus, Action::Back], now);
        assert_eq!(local_titles(&app), ["blue"]);
        // Without a music directory, `/` on the local source explains.
        let (mut nav, now) = nav_app();
        press(&mut nav, &[Action::SelectLocal, Action::OpenSearch], now);
        assert!(!nav.input_open());
        assert_eq!(nav.status(now), Some(NO_LOCAL_LIBRARY));
    }

    // ----- scrobbling ----------------------------------------------------------

    fn timed_remote(id: &str, secs: u64) -> Track {
        Track::navidrome(
            id.into(),
            TrackMetadata {
                title: Some(format!("Remote {id}")),
                duration: Some(Duration::from_secs(secs)),
                ..TrackMetadata::default()
            },
        )
    }

    /// A Navidrome app with `tracks` queued, focused on the queue.
    fn scrobble_app(tracks: Vec<Track>) -> (App, Instant) {
        let (mut app, now) = nav_app();
        app.take_fetch_requests();
        for track in tracks {
            app.player.enqueue(track);
        }
        app.focus = Focus::Queue;
        (app, now)
    }

    /// Reports positions every 250 ms from `from` to `to` seconds.
    fn progress(app: &mut App, from: f64, to: f64, now: Instant) {
        let mut at = from;
        while at < to {
            at = (at + 0.25).min(to);
            app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs_f64(at)), now);
        }
    }

    fn summary(requests: &[ScrobbleRequest]) -> Vec<String> {
        requests
            .iter()
            .map(|r| {
                let kind = if r.submission { "submit" } else { "playing" };
                format!("{kind} {}", r.song_id)
            })
            .collect()
    }

    #[test]
    fn remote_plays_are_reported_and_submitted_once_at_half() {
        let (mut app, now) = scrobble_app(vec![timed_remote("s1", 180), track("local.flac")]);
        let commands = press(&mut app, &[Action::Activate], now);
        assert!(
            app.take_scrobble_requests().is_empty(),
            "not before Started"
        );
        let before = SystemTime::now();
        started(&mut app, &commands, now);
        let after = SystemTime::now();
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing s1"]);
        progress(&mut app, 0.0, 89.75, now);
        assert!(app.take_scrobble_requests().is_empty());
        progress(&mut app, 89.75, 90.0, now);
        let requests = app.take_scrobble_requests();
        assert_eq!(summary(&requests), ["submit s1"]);
        // The wall-clock start of the play, taken at `Started`.
        let started_at = requests[0].started_at.unwrap();
        assert!(before <= started_at && started_at <= after);
        progress(&mut app, 90.0, 180.0, now);
        assert!(app.take_scrobble_requests().is_empty(), "once per load");
        // The local track after it is never scrobbled.
        let load = app.player.current_load().unwrap();
        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 200.0, now);
        assert!(app.take_scrobble_requests().is_empty());
    }

    #[test]
    fn a_zero_server_duration_falls_back_to_the_players() {
        let (mut app, now) = scrobble_app(vec![timed_remote("z", 0)]);
        let commands = press(&mut app, &[Action::Activate], now);
        app.handle_playback_event(PlaybackEvent::Duration(Some(Duration::from_secs(180))), now);
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 89.75, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing z"]);
        progress(&mut app, 89.75, 90.0, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["submit z"]);
    }

    #[test]
    fn seeks_pauses_and_short_tracks_do_not_submit() {
        let (mut app, now) = scrobble_app(vec![timed_remote("s1", 180), timed_remote("short", 29)]);
        let commands = press(&mut app, &[Action::Activate], now);
        started(&mut app, &commands, now);
        app.take_scrobble_requests();
        // Seeking through the track with the arrow keys is not listening.
        let mut at = 0;
        while at < 180 {
            at += 5;
            app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(at)), now);
        }
        assert!(app.take_scrobble_requests().is_empty());
        // Positions while paused do not count either.
        app.handle_playback_event(PlaybackEvent::Position(Duration::ZERO), now);
        press(&mut app, &[Action::TogglePause], now);
        app.handle_playback_event(PlaybackEvent::Paused(true), now);
        progress(&mut app, 0.0, 120.0, now);
        assert!(app.take_scrobble_requests().is_empty());
        // Resumed: 90 s of real listening from here submits.
        press(&mut app, &[Action::TogglePause], now);
        progress(&mut app, 120.0, 210.0, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["submit s1"]);
        // A short track: now playing, but never submitted.
        let commands = press(&mut app, &[Action::Next], now);
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 29.0, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing short"]);
    }

    #[test]
    fn a_new_load_starts_a_new_listen_and_stale_progress_is_ignored() {
        let (mut app, now) = scrobble_app(vec![timed_remote("a", 60), timed_remote("b", 60)]);
        let commands = press(&mut app, &[Action::Activate], now);
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 20.0, now);
        // `n` before the submission: the next load has not started, so
        // positions still arriving for the old track count for nothing.
        let commands = press(&mut app, &[Action::Next], now);
        progress(&mut app, 20.0, 40.0, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing a"]);
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 29.75, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing b"]);
        progress(&mut app, 29.75, 30.0, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["submit b"]);
        // A duplicate Started for the same load changes nothing.
        let load = app.player.current_load().unwrap();
        app.handle_playback_event(PlaybackEvent::Started { load }, now);
        assert!(app.take_scrobble_requests().is_empty());
    }

    #[test]
    fn repeat_one_reloads_are_scrobbled_again() {
        let (mut app, now) = scrobble_app(vec![timed_remote("a", 40)]);
        app.player.set_repeat(crate::player::RepeatMode::One);
        let mut commands = press(&mut app, &[Action::Activate], now);
        for _ in 0..2 {
            let load = started(&mut app, &commands, now);
            progress(&mut app, 0.0, 40.0, now);
            assert_eq!(
                summary(&app.take_scrobble_requests()),
                ["playing a", "submit a"]
            );
            commands = app.handle_playback_event(
                PlaybackEvent::TrackEnded {
                    load,
                    reason: EndReason::Eof,
                },
                now,
            );
        }
    }

    #[test]
    fn scrobbling_can_be_turned_off_and_needs_a_client() {
        let (mut app, now) = scrobble_app(vec![timed_remote("a", 60)]);
        app.set_scrobbling(false);
        let commands = press(&mut app, &[Action::Activate], now);
        started(&mut app, &commands, now);
        progress(&mut app, 0.0, 60.0, now);
        assert!(app.take_scrobble_requests().is_empty());
        // Without a client remote tracks cannot play, and scrobbling
        // cannot be turned on.
        let (mut local, _) = app_with(&[]);
        local.set_scrobbling(true);
        assert!(!local.scrobbling);
    }

    #[test]
    fn only_the_first_scrobble_failure_is_shown() {
        let (mut app, now) = scrobble_app(Vec::new());
        app.handle_scrobble_failure(navidrome::Error::Auth, now);
        assert_eq!(
            app.status(now),
            Some("Navidrome scrobbling failed: authentication failed")
        );
        app.set_status("other", now);
        app.handle_scrobble_failure(navidrome::Error::Http(500), now);
        assert_eq!(app.status(now), Some("other"));
    }

    // ----- queue controls, repeat, session saving -------------------------

    fn queued_app(names: &[&str]) -> (App, Instant) {
        let (mut app, now) = app_with(&[]);
        for name in names {
            app.player.enqueue(track(&format!("{name}.flac")));
        }
        app.focus = Focus::Queue;
        (app, now)
    }

    fn started(app: &mut App, commands: &[PlaybackCommand], now: Instant) -> LoadId {
        let [PlaybackCommand::Load { load, .. }] = commands else {
            panic!("{commands:?}");
        };
        app.handle_playback_event(PlaybackEvent::Started { load: *load }, now);
        *load
    }

    fn reading(value: f32) -> crate::playback::VisualizerFrame {
        crate::playback::VisualizerFrame {
            levels: [value; 2],
            bands: [Some(value); crate::playback::VISUALIZER_BANDS],
        }
    }

    #[test]
    fn v_toggles_the_visualizer_and_the_setting_is_saved_and_restored() {
        let (mut app, now) = queued_app(&["A"]);
        assert!(!app.visualizer_shown(), "off by default");
        let commands = press(&mut app, &[Action::ToggleVisualizer], now);
        assert_eq!(commands, [PlaybackCommand::SetVisualizer(true)]);
        assert!(app.visualizer_shown());
        let saved = app.take_due_save(now + SAVE_DEBOUNCE).expect("saved");
        assert!(saved.visualizer);
        let commands = press(&mut app, &[Action::ToggleVisualizer], now);
        assert_eq!(commands, [PlaybackCommand::SetVisualizer(false)]);
        assert!(!app.take_final_save().expect("changed again").visualizer);

        // Restored on: the backend is told at startup.
        let (mut app, now) = app_with(&[]);
        let commands = app.restore_session(
            Session {
                visualizer: true,
                ..Session::default()
            },
            now,
        );
        assert_eq!(commands, [PlaybackCommand::SetVisualizer(true)]);
        assert!(app.visualizer_shown());
        assert_eq!(app.take_final_save(), None, "restored counts as saved");
        // Restored off: nothing to tell.
        let (mut app, now) = app_with(&[]);
        assert!(app.restore_session(Session::default(), now).is_empty());
        // The key does nothing while help is open.
        press(&mut app, &[Action::ToggleHelp], now);
        assert!(press(&mut app, &[Action::ToggleVisualizer], now).is_empty());
        assert!(!app.visualizer_shown());
    }

    #[test]
    fn readings_redraw_only_while_shown_and_playing_the_current_track() {
        let (mut app, now) = queued_app(&["A", "B"]);
        let commands = press(&mut app, &[Action::Activate], now);
        let load = started(&mut app, &commands, now);
        // Hidden: dropped, nothing to redraw.
        assert!(!app.handle_visualizer(load, &reading(1.0), now));
        assert!(app.meter().is_empty());
        press(&mut app, &[Action::ToggleVisualizer], now);
        assert!(app.handle_visualizer(load, &reading(1.0), now));
        assert!(app.meter().levels()[0] > 0.0);
        // Another load's reading is stale.
        assert!(!app.handle_visualizer(LoadId(load.0 + 100), &reading(1.0), now));
        // Paused: the bars drop and readings are dropped.
        app.handle_playback_event(PlaybackEvent::Paused(true), now);
        assert!(app.meter().is_empty());
        assert!(!app.handle_visualizer(load, &reading(1.0), now));
        app.handle_playback_event(PlaybackEvent::Paused(false), now);
        assert!(app.handle_visualizer(load, &reading(1.0), now));
        // Hidden again: the bars drop at once.
        press(&mut app, &[Action::ToggleVisualizer], now);
        assert!(app.meter().is_empty());
        press(&mut app, &[Action::ToggleVisualizer], now);
        // Next track: loading, so the old load's readings are dropped.
        assert!(app.handle_visualizer(load, &reading(1.0), now));
        press(&mut app, &[Action::Next], now);
        assert!(!app.handle_visualizer(load, &reading(1.0), now));
        // Through the event path as well.
        app.handle_playback_event(
            PlaybackEvent::Visualizer {
                load,
                frame: reading(1.0),
            },
            now,
        );
        assert!(app.meter().is_empty());
        // Stopped.
        press(&mut app, &[Action::ClearQueue, Action::ClearQueue], now);
        assert!(!app.handle_visualizer(load, &reading(1.0), now));
    }

    #[test]
    fn readings_are_requested_only_while_the_visualizer_can_be_seen() {
        let (mut app, now) = queued_app(&["A"]);
        let commands = press(&mut app, &[Action::Activate], now);
        let load = started(&mut app, &commands, now);
        // Not shown: drawability alone asks for nothing.
        assert_eq!(app.set_visualizer_drawable(false), None);
        assert_eq!(app.set_visualizer_drawable(true), None);
        assert_eq!(
            press(&mut app, &[Action::ToggleVisualizer], now),
            [PlaybackCommand::SetVisualizer(true)]
        );
        assert!(app.handle_visualizer(load, &reading(1.0), now));
        // Covered or too small: readings stop and the bars drop.
        assert_eq!(
            app.set_visualizer_drawable(false),
            Some(PlaybackCommand::SetVisualizer(false))
        );
        assert!(app.meter().is_empty());
        assert!(!app.handle_visualizer(load, &reading(1.0), now));
        assert_eq!(app.set_visualizer_drawable(false), None, "no repeats");
        // `v` while covered changes the setting only.
        assert!(press(&mut app, &[Action::ToggleVisualizer], now).is_empty());
        assert!(press(&mut app, &[Action::ToggleVisualizer], now).is_empty());
        assert_eq!(
            app.set_visualizer_drawable(true),
            Some(PlaybackCommand::SetVisualizer(true))
        );
        assert!(app.take_due_save(now + SAVE_DEBOUNCE).unwrap().visualizer);
    }

    #[test]
    fn the_bars_fall_on_the_tick_when_readings_stop() {
        let (mut app, now) = queued_app(&["A"]);
        let commands = press(&mut app, &[Action::Activate], now);
        let load = started(&mut app, &commands, now);
        press(&mut app, &[Action::ToggleVisualizer], now);
        app.handle_visualizer(load, &reading(1.0), now);
        let level = app.meter().levels()[0];
        assert!(!app.tick(now + Duration::from_millis(100)));
        assert!(app.tick(now + Duration::from_millis(400)));
        assert!(app.meter().levels()[0] < level);
        for i in 0..10 {
            app.tick(now + Duration::from_millis(650 + 250 * i));
        }
        assert_eq!(app.meter().levels(), [0.0, 0.0]);
    }

    #[test]
    fn an_unavailable_visualizer_is_reported_once_and_on_v() {
        let (mut app, now) = queued_app(&["A"]);
        press(&mut app, &[Action::ToggleVisualizer], now);
        app.handle_playback_event(PlaybackEvent::VisualizerUnavailable, now);
        assert_eq!(app.status(now), Some(VISUALIZER_UNAVAILABLE));
        assert!(!app.visualizer_shown());
        let later = now + STATUS_TTL * 2;
        app.tick(later);
        app.handle_playback_event(PlaybackEvent::VisualizerUnavailable, later);
        assert_eq!(app.status(later), None, "only once");
        // Turning it off and on again says why nothing shows.
        press(&mut app, &[Action::ToggleVisualizer], later);
        assert_eq!(app.status(later), None);
        press(&mut app, &[Action::ToggleVisualizer], later);
        assert_eq!(app.status(later), Some(VISUALIZER_UNAVAILABLE));
        assert!(!app.visualizer_shown());
    }

    #[test]
    fn move_keys_reorder_and_the_selection_follows() {
        let (mut app, now) = queued_app(&["A", "B", "C"]);
        let commands = press(&mut app, &[Action::Down, Action::Activate], now);
        started(&mut app, &commands, now);
        let load = app.player.current_load();
        assert!(press(&mut app, &[Action::MoveEntryUp], now).is_empty());
        assert_eq!(queue_titles(&app), ["B*", "A", "C"]);
        assert_eq!(app.queue_selected(), 0);
        press(&mut app, &[Action::MoveEntryUp], now);
        assert_eq!(app.queue_selected(), 0, "already first");
        press(
            &mut app,
            &[Action::MoveEntryDown, Action::MoveEntryDown],
            now,
        );
        assert_eq!(queue_titles(&app), ["A", "C", "B*"]);
        assert_eq!(app.queue_selected(), 2);
        press(&mut app, &[Action::MoveEntryDown], now);
        assert_eq!(app.queue_selected(), 2, "already last");
        assert_eq!(app.player.current_load(), load, "playback untouched");
        // Library focus: no effect.
        app.focus = Focus::Library;
        press(&mut app, &[Action::MoveEntryUp], now);
        assert_eq!(queue_titles(&app), ["A", "C", "B*"]);
    }

    #[test]
    fn select_current_jumps_to_the_current_entry() {
        let (mut app, now) = queued_app(&["A", "B", "C"]);
        press(&mut app, &[Action::SelectCurrent], now);
        assert_eq!(app.status(now), Some("No current entry"));
        press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        press(&mut app, &[Action::Up, Action::Up], now);
        assert_eq!(app.queue_selected(), 0);
        assert!(press(&mut app, &[Action::SelectCurrent], now).is_empty());
        assert_eq!(app.queue_selected(), 2);
    }

    #[test]
    fn clear_stops_playback_and_ignores_the_old_end_file() {
        let (mut app, now) = queued_app(&["A", "B"]);
        let commands = press(&mut app, &[Action::Activate], now);
        let load = started(&mut app, &commands, now);
        app.focus = Focus::Library;
        assert!(press(&mut app, &[Action::ClearQueue], now).is_empty());
        assert_eq!(app.player.queue().len(), 2, "queue panel only");
        app.focus = Focus::Queue;
        press(&mut app, &[Action::Down], now);
        assert!(press(&mut app, &[Action::ClearQueue], now).is_empty());
        assert_eq!(app.status(now), Some(CLEAR_PROMPT));
        assert_eq!(app.player.queue().len(), 2, "not yet");
        assert_eq!(
            press(&mut app, &[Action::ClearQueue], now + CLEAR_CONFIRM / 2),
            [PlaybackCommand::Stop]
        );
        assert!(app.player.queue().is_empty());
        assert_eq!(app.queue_selected(), 0);
        assert_eq!(app.status(now), Some("Queue cleared"));
        for reason in [EndReason::Stopped, EndReason::Eof] {
            let commands =
                app.handle_playback_event(PlaybackEvent::TrackEnded { load, reason }, now);
            assert!(commands.is_empty());
        }
        assert_eq!(app.player.status(), crate::player::PlaybackStatus::Stopped);
        // Clearing an empty queue does nothing.
        assert!(press(&mut app, &[Action::ClearQueue], now).is_empty());
    }

    #[test]
    fn shuffle_keeps_the_current_entry_and_playback() {
        let (mut app, now) = queued_app(&["A", "B", "C", "D", "E", "F"]);
        app.rng = Rng::from_seed(11);
        let commands = press(&mut app, &[Action::Down, Action::Activate], now);
        let load = started(&mut app, &commands, now);
        assert!(press(&mut app, &[Action::ShuffleUpcoming], now).is_empty());
        let titles = queue_titles(&app);
        assert_eq!(titles[..2], ["A", "B*"]);
        let mut rest = titles[2..].to_vec();
        rest.sort();
        assert_eq!(rest, ["C", "D", "E", "F"]);
        assert_eq!(app.player.current_load(), Some(load));
        assert_eq!(app.status(now), Some("Shuffled 4 upcoming entries"));
        // The same seed gives the same order.
        let (mut other, _) = queued_app(&["A", "B", "C", "D", "E", "F"]);
        other.rng = Rng::from_seed(11);
        press(&mut other, &[Action::Down, Action::Activate], now);
        press(&mut other, &[Action::ShuffleUpcoming], now);
        assert_eq!(queue_titles(&other), titles);
        // Nothing after the last entry.
        let (mut app, now) = queued_app(&["A"]);
        press(&mut app, &[Action::Activate, Action::ShuffleUpcoming], now);
        assert_eq!(app.status(now), Some("Nothing to shuffle"));
    }

    #[test]
    fn repeat_cycles_from_any_panel_but_not_in_help_or_search() {
        let mut app = App::with_sources(Some(ROOT.into()), Some(client()));
        let now = Instant::now();
        press(&mut app, &[Action::CycleRepeat], now);
        assert_eq!(app.player.repeat(), crate::player::RepeatMode::All);
        assert_eq!(app.status(now), Some("Repeat all"));
        app.focus = Focus::Queue;
        press(&mut app, &[Action::CycleRepeat], now);
        assert_eq!(app.player.repeat(), crate::player::RepeatMode::One);
        press(&mut app, &[Action::ToggleHelp, Action::CycleRepeat], now);
        assert_eq!(app.player.repeat(), crate::player::RepeatMode::One);
        press(
            &mut app,
            &[
                Action::CloseHelp,
                Action::SelectNavidrome,
                Action::OpenSearch,
            ],
            now,
        );
        assert!(app.input_open());
        press(&mut app, &[Action::CycleRepeat], now);
        assert_eq!(app.player.repeat(), crate::player::RepeatMode::One);
    }

    #[test]
    fn saves_are_debounced_and_emitted_as_data() {
        let (mut app, now) = queued_app(&[]);
        assert_eq!(app.take_due_save(now + SAVE_DEBOUNCE * 10), None);
        assert_eq!(app.take_final_save(), None, "nothing changed");

        let (mut app, t0) = app_with(&["a.flac", "b.flac"]);
        press(&mut app, &[Action::Enqueue], t0);
        assert_eq!(app.take_due_save(t0 + SAVE_DEBOUNCE / 2), None);
        // Another change restarts the debounce.
        let t1 = t0 + SAVE_DEBOUNCE / 2;
        press(&mut app, &[Action::CycleRepeat], t1);
        assert_eq!(app.take_due_save(t0 + SAVE_DEBOUNCE), None);
        let session = app.take_due_save(t1 + SAVE_DEBOUNCE).expect("due");
        assert_eq!(session.tracks.len(), 1);
        assert_eq!(session.repeat, crate::player::RepeatMode::All);
        assert_eq!(app.take_due_save(t1 + SAVE_DEBOUNCE * 2), None, "taken");
        assert_eq!(app.take_final_save(), None, "already saved");

        // Progress and pause do not schedule saves.
        let t2 = t1 + SAVE_DEBOUNCE * 3;
        app.focus = Focus::Queue;
        let commands = press(&mut app, &[Action::Activate], t2);
        let load = started(&mut app, &commands, t2);
        let session = app
            .take_due_save(t2 + SAVE_DEBOUNCE)
            .expect("current changed");
        assert_eq!(session.current, Some(0));
        let t3 = t2 + SAVE_DEBOUNCE * 2;
        app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(5)), t3);
        press(&mut app, &[Action::TogglePause], t3);
        assert_eq!(app.take_due_save(t3 + SAVE_DEBOUNCE), None);

        // A pending change is saved at exit even before the debounce.
        press(&mut app, &[Action::VolumeDown], t3);
        let session = app.take_final_save().expect("volume changed");
        assert_eq!(session.volume, 95);
        assert_eq!(app.take_due_save(t3 + SAVE_DEBOUNCE), None, "cancelled");

        // After a failed save, the exit save retries.
        app.save_failed();
        assert!(app.take_final_save().is_some());
        let _ = load;
    }

    #[test]
    fn restore_selects_current_requests_volume_and_counts_as_saved() {
        let (mut app, now) = app_with(&[]);
        let session = Session {
            volume: 60,
            repeat: crate::player::RepeatMode::All,
            current: Some(2),
            tracks: vec![track("a.flac"), remote("r1"), track("c.flac")],
            visualizer: false,
        };
        let commands = app.restore_session(session, now);
        assert_eq!(commands, [PlaybackCommand::SetVolume(60)]);
        assert_eq!(app.queue_selected(), 2);
        assert_eq!(app.player.status(), crate::player::PlaybackStatus::Stopped);
        assert_eq!(app.status(now), Some("Restored queue: 3 tracks"));
        assert_eq!(app.take_due_save(now + SAVE_DEBOUNCE), None);
        assert_eq!(app.take_final_save(), None);
        // mpv reports its startup volume; the restored one stays.
        app.handle_playback_event(PlaybackEvent::Volume(100), now);
        app.handle_playback_event(PlaybackEvent::Volume(60), now);
        assert_eq!(app.player.volume(), 60);
        assert_eq!(app.take_due_save(now + SAVE_DEBOUNCE), None);
        // Restored remote entries without Navidrome fail and advance.
        app.focus = Focus::Queue;
        let commands = press(&mut app, &[Action::Up, Action::Activate], now);
        assert_eq!(load_path(&commands), PathBuf::from("/music/c.flac"));
        assert_eq!(
            app.status(now),
            Some("Could not play Remote r1: Navidrome is not configured")
        );
    }

    #[test]
    fn clear_needs_a_second_press_soon_and_with_nothing_in_between() {
        let (mut app, now) = queued_app(&["A", "B"]);
        // Too late: the second press only asks again.
        press(&mut app, &[Action::ClearQueue], now);
        press(&mut app, &[Action::ClearQueue], now + CLEAR_CONFIRM);
        assert_eq!(app.player.queue().len(), 2);
        assert_eq!(app.status(now + CLEAR_CONFIRM), Some(CLEAR_PROMPT));
        // Another key in between cancels it.
        let t = now + CLEAR_CONFIRM * 3;
        press(
            &mut app,
            &[Action::ClearQueue, Action::Down, Action::ClearQueue],
            t,
        );
        assert_eq!(app.player.queue().len(), 2);
        // Twice in a row clears.
        press(&mut app, &[Action::ClearQueue], t);
        assert!(app.player.queue().is_empty());
        assert_eq!(app.status(t), Some("Queue cleared"));
    }

    #[test]
    fn adds_beyond_the_queue_cap_are_reported() {
        use crate::queue::MAX_QUEUE_LEN;
        let (mut app, now) = app_with(&["dir/a.flac", "dir/b.flac", "dir/c.flac"]);
        let filler: Vec<Track> = (0..MAX_QUEUE_LEN - 1)
            .map(|i| track(&format!("f/{i}.flac")))
            .collect();
        app.restore_session(
            Session {
                tracks: filler,
                ..Session::default()
            },
            now,
        );
        // `a` on the directory: one fits.
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(app.player.queue().len(), MAX_QUEUE_LEN);
        assert_eq!(
            app.status(now),
            Some("Added 1 track; queue full, 2 not added")
        );
        press(&mut app, &[Action::Enqueue], now);
        assert_eq!(app.status(now), Some(QUEUE_FULL));
        // Enter on a track: refused, nothing loads.
        press(&mut app, &[Action::Activate], now);
        let commands = press(&mut app, &[Action::Activate], now);
        assert!(commands.is_empty(), "{commands:?}");
        assert_eq!(app.status(now), Some(QUEUE_FULL));
        assert_eq!(app.player.queue().len(), MAX_QUEUE_LEN);
    }

    #[test]
    fn help_scrolls_within_the_bindings() {
        let (mut app, now) = app_with(&[]);
        press(&mut app, &[Action::ToggleHelp, Action::Up], now);
        assert_eq!(app.help_scroll(), 0);
        for _ in 0..100 {
            press(&mut app, &[Action::Down], now);
        }
        assert_eq!(app.help_scroll(), help::BINDINGS.len() - 1);
        app.clamp_help_scroll(4);
        press(&mut app, &[Action::Up], now);
        assert_eq!(app.help_scroll(), 3, "responds at once after clamping");
        assert_eq!(app.queue_selected(), 0, "the panels did not move");
        press(&mut app, &[Action::CloseHelp, Action::ToggleHelp], now);
        assert_eq!(app.help_scroll(), 0, "reopening starts at the top");
    }

    // ----- Saver -----------------------------------------------------------

    use std::sync::Mutex;
    use std::sync::mpsc as std_mpsc;

    /// A writer that records sessions and blocks until released (one
    /// `true`/`false` per write: succeed or fail).
    struct GatedWriter {
        written: Arc<Mutex<Vec<Session>>>,
        started: Arc<Mutex<usize>>,
        release: std_mpsc::Sender<bool>,
    }

    fn gated_saver(path: Option<PathBuf>) -> (Saver, GatedWriter) {
        let (release, gate) = std_mpsc::channel::<bool>();
        let gate = Arc::new(Mutex::new(gate));
        let written = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(Mutex::new(0));
        let (w, st) = (written.clone(), started.clone());
        let write: SaveFn = Arc::new(move |_path: &Path, session: &Session| {
            *st.lock().unwrap() += 1;
            let ok = gate.lock().unwrap().recv().unwrap_or(false);
            if ok {
                w.lock().unwrap().push(session.clone());
                Ok(())
            } else {
                Err(io::Error::other("disk full"))
            }
        });
        let saver = Saver::with_writer(path, write);
        (
            saver,
            GatedWriter {
                written,
                started,
                release,
            },
        )
    }

    async fn settle(saver: &Saver) {
        if let Some(handle) = &saver.in_flight {
            while !handle.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    fn changed_app(names: &[&str], now: Instant) -> App {
        let (mut app, _) = queued_app(&[]);
        for name in names {
            app.player.enqueue(track(name));
        }
        app.note_changes(now);
        app
    }

    #[tokio::test]
    async fn saver_writes_one_snapshot_at_a_time() {
        let now = Instant::now();
        let (mut saver, w) = gated_saver(Some("/unused/state.json".into()));
        let mut app = changed_app(&["a"], now);
        saver.poll(&mut app, now);
        assert!(saver.in_flight.is_none(), "not due yet");
        let due = now + SAVE_DEBOUNCE;
        saver.poll(&mut app, due);
        assert!(saver.in_flight.is_some());
        // A second change becomes due while the first write blocks.
        app.player.enqueue(track("b"));
        app.note_changes(due);
        let later = due + SAVE_DEBOUNCE;
        saver.poll(&mut app, later);
        saver.poll(&mut app, later);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*w.started.lock().unwrap(), 1, "still one write");
        w.release.send(true).unwrap();
        settle(&saver).await;
        saver.poll(&mut app, later);
        assert!(saver.in_flight.is_some(), "the waiting save starts");
        w.release.send(true).unwrap();
        settle(&saver).await;
        saver.poll(&mut app, later);
        assert!(saver.in_flight.is_none());
        let written = w.written.lock().unwrap();
        let lens: Vec<usize> = written.iter().map(|s| s.tracks.len()).collect();
        assert_eq!(lens, [1, 2], "in order");
    }

    #[tokio::test]
    async fn exit_save_waits_for_a_running_write_then_saves_the_rest() {
        let now = Instant::now();
        let (mut saver, w) = gated_saver(Some("/unused/state.json".into()));
        let mut app = changed_app(&["a"], now);
        saver.poll(&mut app, now + SAVE_DEBOUNCE);
        app.player.enqueue(track("b"));
        let release = w.release.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            release.send(true).unwrap();
            release.send(true).unwrap();
        });
        saver.finish(&mut app).await;
        let lens: Vec<usize> = w
            .written
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.tracks.len())
            .collect();
        assert_eq!(lens, [1, 2]);
    }

    #[tokio::test]
    async fn a_failed_write_is_retried_at_exit() {
        let now = Instant::now();
        let (mut saver, w) = gated_saver(Some("/unused/state.json".into()));
        let mut app = changed_app(&["a"], now);
        saver.poll(&mut app, now + SAVE_DEBOUNCE);
        w.release.send(false).unwrap();
        settle(&saver).await;
        saver.poll(&mut app, now + SAVE_DEBOUNCE);
        assert!(saver.in_flight.is_none(), "collected");
        assert!(w.written.lock().unwrap().is_empty());
        w.release.send(true).unwrap();
        saver.finish(&mut app).await;
        assert_eq!(w.written.lock().unwrap().len(), 1, "retried");
        assert_eq!(*w.started.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn saver_without_a_path_never_writes() {
        let now = Instant::now();
        let (mut saver, w) = gated_saver(None);
        let mut app = changed_app(&["a"], now);
        saver.poll(&mut app, now + SAVE_DEBOUNCE);
        assert!(saver.in_flight.is_none());
        saver.finish(&mut app).await;
        assert_eq!(*w.started.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn saver_writes_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda").join("state.json");
        let now = Instant::now();
        let saver = Saver::new(Some(path.clone()));
        let mut app = changed_app(&["a", "b"], now);
        saver.finish(&mut app).await;
        let (session, _) = session::load(&path);
        assert_eq!(session.tracks.len(), 2);
    }

    // ----- settings and hot swap ----------------------------------------------

    fn settings_context(music: Option<&str>, navidrome: settings::Navidrome) -> settings::Context {
        settings::Context {
            config_path: Some("/cfg/config.toml".into()),
            music_dir: music.map(PathBuf::from),
            music_origin: None,
            file_has_music_dir: music.is_some(),
            navidrome,
            env_overrides: Vec::new(),
        }
    }

    /// The account of [`client`], as Coda wrote it.
    fn coda_owned() -> settings::Navidrome {
        settings::Navidrome::Coda {
            url: navidrome::client::parse_base_url("https://h.example/nd").unwrap(),
            username: "u".into(),
            account: "u@h.example".into(),
        }
    }

    fn other_client() -> navidrome::Client {
        navidrome::Client::new(
            navidrome::client::parse_base_url("https://other.example/nd").unwrap(),
            "v".into(),
            navidrome::Password::new("q".into()),
        )
        .unwrap()
    }

    fn settings_type(app: &mut App, text: &str, now: Instant) {
        for c in text.chars() {
            app.handle_settings_input(Input::Char(c), now);
        }
        app.handle_settings_input(Input::Enter, now);
    }

    /// Connects (or, for a Coda-owned server, edits) through the overlay
    /// up to the save; the loop's answers are faked. Returns the save's id.
    fn settings_up_to_save(app: &mut App, now: Instant) -> settings::RequestId {
        app.handle_action(Action::OpenSettings, now);
        assert!(app.settings_open());
        if matches!(app.settings().navidrome(), settings::Navidrome::Coda { .. }) {
            app.handle_settings_input(Input::Char('e'), now);
        } else {
            app.handle_settings_input(Input::Char('c'), now);
            settings_type(app, "other.example/nd", now);
            settings_type(app, "v", now);
        }
        settings_type(app, "q", now);
        let requests = app.take_settings_requests();
        let [settings::Request::Ping { id, .. }] = requests.as_slice() else {
            panic!("expected a ping, got {requests:?}");
        };
        app.handle_settings_response(
            settings::Response::Pinged {
                id: *id,
                result: Ok(()),
            },
            now,
        );
        let requests = app.take_settings_requests();
        let [settings::Request::Save { id, .. }] = requests.as_slice() else {
            panic!("expected a save, got {requests:?}");
        };
        *id
    }

    /// Connects or edits through the overlay, ending with `client`.
    fn connect_in_app(app: &mut App, client: navidrome::Client, scrobble: bool, now: Instant) {
        let id = settings_up_to_save(app, now);
        app.handle_settings_response(
            settings::Response::Saved {
                id,
                result: Ok(settings::Saved { client, scrobble }),
            },
            now,
        );
    }

    fn stream_host(commands: &[PlaybackCommand]) -> String {
        match commands {
            [
                PlaybackCommand::Load {
                    target: PlaybackTarget::Stream(url),
                    ..
                },
            ] => url::Url::parse(url.expose())
                .unwrap()
                .host_str()
                .unwrap()
                .to_owned(),
            other => panic!("expected a single stream Load, got {other:?}"),
        }
    }

    #[test]
    fn connecting_in_the_app_keeps_playback_and_browses_the_new_server() {
        let (mut app, now) = app_with(&["a.flac"]);
        app.set_settings_context(settings_context(
            Some(ROOT),
            settings::Navidrome::NotConfigured,
        ));
        let commands = press(&mut app, &[Action::Activate], now);
        let load = started(&mut app, &commands, now);
        assert_eq!(app.player().status(), PlaybackStatus::Playing);

        connect_in_app(&mut app, other_client(), true, now);
        let change = app.take_client_change().expect("a client change");
        assert!(change.client.is_some());
        assert!(change.scrobble);
        assert!(app.take_client_change().is_none(), "taken once");
        // Playback is untouched.
        assert_eq!(app.player().status(), PlaybackStatus::Playing);
        assert_eq!(app.player().current_load(), Some(load));
        assert_eq!(app.connection(), &Connection::Connecting);
        assert!(matches!(
            app.settings().navidrome(),
            settings::Navidrome::Coda { .. }
        ));
        assert!(app.settings().form().is_none());
        assert_eq!(
            app.status(now),
            Some("Navidrome saved; connecting as v @ other.example")
        );

        app.handle_settings_input(Input::Esc, now);
        assert!(!app.settings_open());
        press(&mut app, &[Action::SelectNavidrome], now);
        assert!(requests(&mut app).is_empty(), "Home fetches nothing");
        press(&mut app, &[Action::Down, Action::Activate], now);
        let request = one_request(&mut app);
        assert_eq!(request.generation, Generation(1));
        assert_eq!(request.resource, Resource::Artists);
        respond(&mut app, &request, Ok(Content::Artists(vec![artist("b1")])));
        assert_eq!(current_load(&app), Load::Ready);
        assert_eq!(app.connection(), &Connection::Connected);
    }

    #[test]
    fn a_stale_response_from_the_old_client_is_ignored_after_the_swap() {
        let (mut app, now) = nav_app();
        app.set_settings_context(settings_context(None, coda_owned()));
        let old = one_request(&mut app);
        assert_eq!(old.generation, Generation(0));
        // Browse a little first: the new browser starts over, on Home.
        connect_in_app(&mut app, other_client(), true, now);
        assert!(view_titles(&app).is_empty());
        assert!(requests(&mut app).is_empty());
        app.handle_settings_input(Input::Esc, now);
        press(&mut app, &[Action::Down, Action::Activate], now);
        let new = one_request(&mut app);
        assert_eq!(new.generation, Generation(1));
        assert_eq!(
            new.id, old.id,
            "ids restart, so only the generation tells them apart"
        );
        assert_eq!(view_titles(&app), ["Artists"]);

        respond(&mut app, &old, Ok(Content::Artists(vec![artist("old")])));
        assert_eq!(current_load(&app), Load::Loading(new.id));
        assert!(browser(&app).cached(&Resource::Artists).is_none());
        assert_eq!(app.connection(), &Connection::Connecting);
        // An old failure does not fail the new view either.
        respond(&mut app, &old, Err(navidrome::Error::Auth));
        assert_eq!(current_load(&app), Load::Loading(new.id));

        respond(&mut app, &new, Ok(Content::Artists(vec![artist("new")])));
        assert_eq!(current_load(&app), Load::Ready);
        let Some(Content::Artists(artists)) = browser(&app).cached(&Resource::Artists) else {
            panic!()
        };
        assert_eq!(artists[0].id, "new");
    }

    #[test]
    fn the_swap_resets_the_browser() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        app.set_settings_context(settings_context(None, coda_owned()));
        press(&mut app, &[Action::Activate], now);
        assert_eq!(view_titles(&app), ["Artists", "Artist a1"]);
        requests(&mut app);
        connect_in_app(&mut app, other_client(), true, now);
        assert!(view_titles(&app).is_empty(), "back on Home");
        assert!(browser(&app).cached(&Resource::Artists).is_none());
        assert_eq!(browser(&app).generation(), Generation(1));
        assert!(requests(&mut app).is_empty());
    }

    #[test]
    fn the_swap_drops_a_listen_in_progress_and_replaces_the_scrobbler() {
        let (mut app, now) = scrobble_app(vec![timed_remote("s1", 180)]);
        app.set_settings_context(settings_context(None, coda_owned()));
        let commands = press(&mut app, &[Action::Activate], now);
        started(&mut app, &commands, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing s1"]);
        progress(&mut app, 0.0, 30.0, now);
        assert!(app.take_scrobble_requests().is_empty());

        connect_in_app(&mut app, other_client(), true, now);
        assert!(app.take_client_change().unwrap().scrobble);
        // The listen belonged to the old server: its submission never
        // goes to the new one.
        progress(&mut app, 30.0, 170.0, now);
        assert!(app.take_scrobble_requests().is_empty());
        assert_eq!(app.player().status(), PlaybackStatus::Playing);

        // A file with `scrobble = false` turns scrobbling off.
        connect_in_app(&mut app, client(), false, now);
        let change = app.take_client_change().unwrap();
        assert!(change.client.is_some());
        assert!(!change.scrobble);
        assert!(!app.scrobbling);
    }

    #[test]
    fn a_queued_remote_track_resolves_with_the_new_client() {
        let (mut app, now) = scrobble_app(vec![remote("r1"), remote("r2")]);
        app.set_settings_context(settings_context(None, coda_owned()));
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_host(&commands), "h.example");
        let load = started(&mut app, &commands, now);

        connect_in_app(&mut app, other_client(), true, now);
        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        assert_eq!(stream_host(&commands), "other.example");
        assert_eq!(stream_id(&commands), "r2");
    }

    #[test]
    fn after_disconnect_remote_tracks_fail_and_the_queue_advances() {
        let now = Instant::now();
        let mut app = App::with_sources(Some(ROOT.into()), Some(client()));
        app.set_settings_context(settings_context(Some(ROOT), coda_owned()));
        app.player.enqueue(remote("r1"));
        app.player.enqueue(remote("r2"));
        app.player.enqueue(track("b.flac"));
        app.focus = Focus::Queue;
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_id(&commands), "r1");
        let load = started(&mut app, &commands, now);
        press(&mut app, &[Action::SelectNavidrome], now);
        requests(&mut app);

        app.handle_action(Action::OpenSettings, now);
        app.handle_settings_input(Input::Char('d'), now);
        app.handle_settings_input(Input::Char('d'), now);
        let requests = app.take_settings_requests();
        let [
            settings::Request::Disconnect {
                id,
                previous_account,
            },
        ] = requests.as_slice()
        else {
            panic!("expected a disconnect, got {requests:?}");
        };
        assert_eq!(previous_account.as_deref(), Some("u@h.example"));
        app.handle_settings_response(
            settings::Response::Disconnected {
                id: *id,
                result: Ok(()),
            },
            now,
        );
        let change = app.take_client_change().unwrap();
        assert!(change.client.is_none());
        assert!(!change.scrobble);
        assert!(app.browser().is_none());
        assert_eq!(app.source(), Source::Local);
        // The current stream plays on.
        assert_eq!(app.player().status(), PlaybackStatus::Playing);

        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        assert_eq!(load_path(&commands), PathBuf::from("/music/b.flac"));
        assert_eq!(
            app.status(now),
            Some("Could not play Remote r2: Navidrome is not configured")
        );
    }

    #[test]
    fn a_second_quit_while_saving_quits() {
        let (mut app, now) = app_with(&[]);
        app.set_settings_context(settings_context(
            Some(ROOT),
            settings::Navidrome::NotConfigured,
        ));
        let id = settings_up_to_save(&mut app, now);
        app.handle_settings_input(Input::Quit, now);
        assert!(!app.should_quit());
        assert_eq!(app.status(now), Some(SAVING_SETTINGS));
        // The save ends (failing): the warning is reset, so quitting
        // during the next save warns again.
        app.handle_settings_response(
            settings::Response::Saved {
                id,
                result: Err("Could not write /cfg/config.toml: denied".into()),
            },
            now,
        );
        // The form stays open with the error; `q` is text there.
        app.handle_settings_input(Input::Char('q'), now);
        assert!(!app.should_quit());
        app.handle_settings_input(Input::Enter, now);
        let requests = app.take_settings_requests();
        let [settings::Request::Ping { id, .. }] = requests.as_slice() else {
            panic!("{requests:?}");
        };
        app.handle_settings_response(
            settings::Response::Pinged {
                id: *id,
                result: Ok(()),
            },
            now,
        );
        assert_eq!(app.settings().busy(), Some(settings::Busy::Saving));
        app.handle_settings_input(Input::Quit, now);
        assert!(!app.should_quit(), "warned again");
        app.handle_settings_input(Input::Quit, now);
        assert!(app.should_quit(), "the second Ctrl-C quits");
    }

    #[test]
    fn a_load_still_loading_at_the_swap_is_not_scrobbled_to_the_new_server() {
        let (mut app, now) = scrobble_app(vec![timed_remote("s1", 180), timed_remote("s2", 180)]);
        app.set_settings_context(settings_context(None, coda_owned()));
        // Resolved (and sent to mpv) with the old client...
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(stream_host(&commands), "h.example");
        // ...then the swap lands before it starts.
        connect_in_app(&mut app, other_client(), true, now);
        assert!(app.take_client_change().unwrap().scrobble);
        let load = started(&mut app, &commands, now);
        progress(&mut app, 0.0, 170.0, now);
        assert!(app.take_scrobble_requests().is_empty());
        // The next load is the new server's and is reported.
        let commands = app.handle_playback_event(
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
            now,
        );
        assert_eq!(stream_host(&commands), "other.example");
        started(&mut app, &commands, now);
        assert_eq!(summary(&app.take_scrobble_requests()), ["playing s2"]);
    }

    #[test]
    fn the_old_ping_racing_the_swap_is_ignored() {
        let (mut app, now) = nav_app();
        app.set_settings_context(settings_context(None, coda_owned()));
        requests(&mut app);
        connect_in_app(&mut app, other_client(), true, now);
        let change = app.take_client_change().unwrap();
        assert_eq!(change.generation, Generation(1));
        // The startup ping of the old client answers late.
        app.handle_ping(Generation(0), Err(navidrome::Error::Auth), now);
        assert_eq!(app.connection(), &Connection::Connecting);
        app.handle_ping(Generation(0), Ok(()), now);
        assert_eq!(app.connection(), &Connection::Connecting);
        app.handle_ping(Generation(1), Err(navidrome::Error::Auth), now);
        assert_eq!(
            app.connection(),
            &Connection::Failed("authentication failed".into())
        );
    }

    #[test]
    fn q_quits_from_the_settings_overview_and_actions_are_swallowed() {
        let (mut app, now) = app_with(&["a.flac"]);
        app.handle_action(Action::OpenSettings, now);
        assert!(app.settings_open());
        assert!(press(&mut app, &[Action::Activate, Action::Next], now).is_empty());
        app.handle_settings_input(Input::Char('q'), now);
        assert!(app.should_quit());
    }

    // ----- settings I/O (mock store, temporary files, fake server) -------------

    fn store_settings() -> Result<SecretStore, SecretError> {
        static STORE: std::sync::OnceLock<SecretStore> = std::sync::OnceLock::new();
        Ok(STORE.get_or_init(SecretStore::mock).clone())
    }

    fn server(url: &str, username: &str, password: &str) -> crate::onboarding::Server {
        crate::onboarding::Server {
            url: navidrome::client::parse_base_url(url).unwrap(),
            username: username.into(),
            password: navidrome::Password::new(password.into()),
        }
    }

    async fn perform(io: &mut SettingsIo, request: settings::Request) -> settings::Response {
        let (tx, mut rx) = mpsc::unbounded_channel();
        io.spawn(request, &tx);
        rx.recv().await.unwrap()
    }

    #[tokio::test]
    async fn settings_saves_keep_other_settings_and_replace_the_account() {
        use crate::config::{Cli, Config, Env, PasswordSource};
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let path = dir.path().join("coda").join("config.toml");
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        let old = format!(
            "music_dir = \"{}\"\n\n[navidrome]\nurl = \"https://old.example\"\nusername = \"me\"\npassword_keychain = true\nscrobble = false\n",
            music.display()
        );
        std::fs::write(&path, &old).unwrap();
        let store = store_settings().unwrap();
        store
            .set("me@old.example", &navidrome::Password::new("old-pw".into()))
            .unwrap();
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let mut io = SettingsIo::new(Some(path.clone()), store_settings);

        // Edit to another account.
        let response = perform(
            &mut io,
            settings::Request::Save {
                id: 1,
                server: server("https://new.example:8443", "you", "new-pw"),
                previous_account: Some("me@old.example".into()),
            },
        )
        .await;
        let settings::Response::Saved {
            id: 1,
            result: Ok(saved),
        } = response
        else {
            panic!("{response:?}");
        };
        assert!(!saved.scrobble, "the file's scrobble = false is kept");
        assert_eq!(
            saved.client.base_url().as_str(),
            "https://new.example:8443/"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("new-pw"), "{text}");
        assert!(text.contains("scrobble = false"), "{text}");
        let config = Config::load(Cli::default(), &env).unwrap();
        assert_eq!(config.music_dir.unwrap(), music.canonicalize().unwrap());
        let nd = config.navidrome.unwrap();
        assert_eq!(nd.url.as_str(), "https://new.example:8443/");
        assert!(!nd.scrobble);
        let PasswordSource::Keychain { account, .. } = &nd.password else {
            panic!()
        };
        assert_eq!(account, "you@new.example:8443");
        assert_eq!(store.get(account).unwrap().expose(), "new-pw");
        assert_eq!(store.get("me@old.example"), Err(SecretError::Missing));
        // The previous file was backed up.
        let backups = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".bak.")
            })
            .count();
        assert!(backups >= 1);

        // Disconnect: `[navidrome]` and the entry go, the folder stays.
        let response = perform(
            &mut io,
            settings::Request::Disconnect {
                id: 2,
                previous_account: Some("you@new.example:8443".into()),
            },
        )
        .await;
        assert!(
            matches!(
                response,
                settings::Response::Disconnected {
                    id: 2,
                    result: Ok(())
                }
            ),
            "{response:?}"
        );
        let config = Config::load(Cli::default(), &env).unwrap();
        assert!(config.navidrome.is_none());
        assert!(config.music_dir.is_some());
        assert_eq!(store.get("you@new.example:8443"), Err(SecretError::Missing));
        io.finish(SETTINGS_SAVE_WAIT).await;
    }

    #[tokio::test]
    async fn a_failed_settings_write_keeps_the_old_file_and_entry() {
        fn store() -> Result<SecretStore, SecretError> {
            static STORE: std::sync::OnceLock<SecretStore> = std::sync::OnceLock::new();
            Ok(STORE.get_or_init(SecretStore::mock).clone())
        }
        let dir = tempfile::tempdir().unwrap();
        // The config's directory is a file: the write fails.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let path = blocker.join("config.toml");
        let mut io = SettingsIo::new(Some(path), store);
        store()
            .unwrap()
            .set("me@old.example", &navidrome::Password::new("old".into()))
            .unwrap();
        let response = perform(
            &mut io,
            settings::Request::Save {
                id: 7,
                server: server("https://new.example", "you", "pw"),
                previous_account: Some("me@old.example".into()),
            },
        )
        .await;
        let settings::Response::Saved {
            id: 7,
            result: Err(message),
        } = response
        else {
            panic!("{response:?}");
        };
        assert!(message.starts_with("Could not write"), "{message}");
        // The message names the (random) temporary path, which may itself
        // contain "pw"; only the rest must be free of secrets.
        let message = message.replace(&dir.path().display().to_string(), "<dir>");
        for secret in ["pw", "you@", "new.example", "old.example"] {
            assert!(!message.contains(secret), "{secret}: {message}");
        }
        // The new entry was removed again; the old one is still used.
        assert_eq!(
            store().unwrap().get("you@new.example"),
            Err(SecretError::Missing)
        );
        assert_eq!(
            store().unwrap().get("me@old.example").unwrap().expose(),
            "old"
        );
        // Without a config location nothing is attempted.
        let mut nowhere = SettingsIo::new(None, store);
        let response = perform(
            &mut nowhere,
            settings::Request::Disconnect {
                id: 8,
                previous_account: None,
            },
        )
        .await;
        assert!(matches!(
            response,
            settings::Response::Disconnected {
                id: 8,
                result: Err(_)
            }
        ));
    }

    #[tokio::test]
    async fn settings_pings_against_a_fake_server() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let ok = json!({ "subsonic-response": { "status": "ok", "version": "1.16.1" } });
        let auth = json!({ "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": 40, "message": "Wrong username or password" }
        } });
        for (body, expected) in [
            (ok, Ok(())),
            (auth, Err("Authentication failed".to_owned())),
        ] {
            let mock = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/ping"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&mock)
                .await;
            let mut io = SettingsIo::new(None, store_settings);
            let response = perform(
                &mut io,
                settings::Request::Ping {
                    id: 3,
                    server: server(&mock.uri(), "me", "pass-word"),
                },
            )
            .await;
            let settings::Response::Pinged { id: 3, result } = response else {
                panic!("{response:?}");
            };
            assert_eq!(result, expected);
            for request in mock.received_requests().await.unwrap() {
                assert!(!request.url.as_str().contains("pass-word"));
            }
        }
    }

    #[tokio::test]
    async fn the_old_clients_fetches_are_aborted_and_never_block_the_new_one() {
        use serde_json::json;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let artists = json!({ "subsonic-response": {
            "status": "ok", "version": "1.16.1", "artists": { "index": [] }
        } });
        let slow = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(artists.clone())
                    .set_delay(Duration::from_secs(20)),
            )
            .mount(&slow)
            .await;
        let fast = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(artists))
            .mount(&fast)
            .await;
        let client_for = |server: &MockServer| {
            navidrome::Client::new(
                navidrome::client::parse_base_url(&server.uri()).unwrap(),
                "u".into(),
                navidrome::Password::new("p".into()),
            )
            .unwrap()
        };
        let request = |generation, id| FetchRequest {
            generation: Generation(generation),
            id: browse::RequestId(id),
            resource: Resource::Search(format!("q{id}")),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut fetches = Fetches::new();
        // More than the permits: some run (and hang), the rest queue.
        let old: Vec<_> = (1..=MAX_CONCURRENT_FETCHES as u64 + 2)
            .map(|id| request(0, id))
            .collect();
        fetches.spawn(old, Some(&client_for(&slow)), &tx);
        tokio::time::sleep(Duration::from_millis(300)).await;
        fetches.restart();
        fetches.spawn(vec![request(1, 1)], Some(&client_for(&fast)), &tx);
        let mut aborted = 0;
        let new = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response = rx.recv().await.unwrap();
                if response.generation == Generation(1) {
                    return response;
                }
                assert!(matches!(response.result, Err(navidrome::Error::Aborted)));
                aborted += 1;
            }
        })
        .await
        .expect("the new client's fetch was not held up");
        assert!(new.result.is_ok(), "{:?}", new.result);
        // Every old fetch answers (aborted), and none ran after the swap.
        while aborted < MAX_CONCURRENT_FETCHES + 2 {
            let response = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.generation, Generation(0));
            assert!(matches!(response.result, Err(navidrome::Error::Aborted)));
            aborted += 1;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let reached = slow.received_requests().await.unwrap().len();
        assert!(reached <= MAX_CONCURRENT_FETCHES, "{reached}");
    }

    #[tokio::test]
    async fn connect_from_not_configured_writes_navidrome_next_to_the_folder() {
        use crate::config::{Cli, Config, Env, PasswordSource};
        fn store() -> Result<SecretStore, SecretError> {
            static STORE: std::sync::OnceLock<SecretStore> = std::sync::OnceLock::new();
            Ok(STORE.get_or_init(SecretStore::mock).clone())
        }
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let path = dir.path().join("config.toml");
        let old = format!("music_dir = \"{}\"\n", music.display());
        std::fs::write(&path, &old).unwrap();
        let mut io = SettingsIo::new(Some(path.clone()), store);
        let response = perform(
            &mut io,
            settings::Request::Save {
                id: 1,
                server: server("http://127.0.0.1:4533", "me", "pw"),
                previous_account: None,
            },
        )
        .await;
        let settings::Response::Saved {
            id: 1,
            result: Ok(saved),
        } = response
        else {
            panic!("{response:?}");
        };
        assert!(saved.scrobble, "the default");
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let config = Config::load(Cli::default(), &env).unwrap();
        assert_eq!(config.music_dir.unwrap(), music.canonicalize().unwrap());
        let nd = config.navidrome.unwrap();
        assert!(nd.scrobble);
        let PasswordSource::Keychain { account, .. } = &nd.password else {
            panic!()
        };
        assert_eq!(account, "me@127.0.0.1:4533");
        assert_eq!(store().unwrap().get(account).unwrap().expose(), "pw");
        // The hand-written file was backed up; Coda's own is not, so a
        // second save adds no backup.
        let backups = || {
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .contains(".bak.")
                })
                .count()
        };
        assert_eq!(backups(), 1);
        perform(
            &mut io,
            settings::Request::Save {
                id: 2,
                server: server("http://127.0.0.1:4533", "me", "pw2"),
                previous_account: Some("me@127.0.0.1:4533".into()),
            },
        )
        .await;
        assert_eq!(backups(), 1);
        assert_eq!(store().unwrap().get(account).unwrap().expose(), "pw2");
    }

    #[tokio::test]
    async fn disconnect_keeps_the_entry_when_the_file_has_no_folder() {
        fn store() -> Result<SecretStore, SecretError> {
            static STORE: std::sync::OnceLock<SecretStore> = std::sync::OnceLock::new();
            Ok(STORE.get_or_init(SecretStore::mock).clone())
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[navidrome]\nurl = \"https://h.example\"\nusername = \"me\"\npassword_keychain = true\n";
        std::fs::write(&path, text).unwrap();
        store()
            .unwrap()
            .set("me@h.example", &navidrome::Password::new("pw".into()))
            .unwrap();
        let mut io = SettingsIo::new(Some(path.clone()), store);
        let response = perform(
            &mut io,
            settings::Request::Disconnect {
                id: 1,
                previous_account: Some("me@h.example".into()),
            },
        )
        .await;
        let settings::Response::Disconnected {
            id: 1,
            result: Err(message),
        } = response
        else {
            panic!("{response:?}");
        };
        assert!(
            message.contains(crate::config_writer::NO_SOURCE_LEFT),
            "{message}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(store().unwrap().get("me@h.example").unwrap().expose(), "pw");
    }

    // ----- Navidrome playlist editing ------------------------------------

    use crate::navidrome::browse::{
        INCOMPLETE_PLAYLIST, MODIFIED, READ_ONLY_FLAGGED, READ_ONLY_OTHER, READ_ONLY_UNKNOWN,
        STALE_PLAYLIST,
    };
    use crate::navidrome::edit::{EditOp, EditResult};
    use crate::navidrome::{Access, Playlist, PlaylistSongs};

    fn owned_by(owner: Option<&str>, readonly: bool) -> Access {
        Access {
            owner: owner.map(str::to_owned),
            readonly,
        }
    }

    /// A playlist of the test client's user (`u`; the server may spell it
    /// in another case).
    fn own(id: &str, name: &str) -> Playlist {
        Playlist {
            id: id.into(),
            name: name.into(),
            song_count: None,
            duration: None,
            access: owned_by(Some("U"), false),
        }
    }

    /// Another user's playlist, flagged read-only as Navidrome does.
    fn others(id: &str, name: &str) -> Playlist {
        Playlist {
            access: owned_by(Some("someone"), true),
            ..own(id, name)
        }
    }

    fn playlist_songs(ids: &[&str], access: Access) -> Content {
        Content::PlaylistSongs(PlaylistSongs {
            tracks: ids.iter().map(|id| remote(id)).collect(),
            served: ids.len(),
            access,
            song_count: None,
        })
    }

    fn own_songs(ids: &[&str]) -> Content {
        playlist_songs(ids, owned_by(Some("u"), false))
    }

    /// Navidrome only, in the Playlists list holding `lists`.
    fn playlists_app(lists: Vec<Playlist>) -> (App, Instant) {
        let (mut app, now) = nav_home();
        press(&mut app, &[Action::Activate], now);
        let request = one_request(&mut app);
        assert_eq!(request.resource, Resource::Playlists);
        respond(&mut app, &request, Ok(Content::Playlists(lists)));
        (app, now)
    }

    /// Opens the selected playlist and answers it with `songs`.
    fn open_playlist(app: &mut App, songs: Content, now: Instant) {
        press(app, &[Action::Activate], now);
        let request = one_request(app);
        assert!(matches!(request.resource, Resource::Playlist(_)));
        respond(app, &request, Ok(songs));
    }

    fn one_edit(app: &mut App) -> EditRequest {
        let mut edits = app.take_edit_requests();
        assert_eq!(edits.len(), 1, "{edits:?}");
        edits.remove(0)
    }

    fn answer_edit(app: &mut App, request: &EditRequest, result: EditResult) {
        app.handle_edit_response(
            EditResponse {
                generation: request.generation,
                id: request.id,
                result,
            },
            Instant::now(),
        );
    }

    fn status_text(app: &App, now: Instant) -> String {
        app.status(now).unwrap_or_default().to_owned()
    }

    fn shown_title(app: &App) -> String {
        let browser = browser(app);
        browser.title(browser.current().unwrap())
    }

    fn song_titles(app: &App) -> Vec<String> {
        browser(app)
            .current_content()
            .unwrap()
            .rows(0, usize::MAX)
            .into_iter()
            .map(|row| match row {
                browse::Row::Song(t) => t.title.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn server_error(message: &str) -> navidrome::Error {
        navidrome::Error::Server {
            code: 0,
            message: message.into(),
        }
    }

    #[test]
    fn playlists_of_other_users_are_read_only_with_a_short_message() {
        let (mut app, now) = playlists_app(vec![
            others("p1", "Theirs"),
            own("p2", "Flagged"),
            own("p3", "Unknown"),
        ]);
        press(&mut app, &[Action::RenamePlaylist], now);
        assert!(browser(&app).prompt().is_none());
        assert_eq!(status_text(&app, now), READ_ONLY_OTHER);
        press(
            &mut app,
            &[Action::DeletePlaylist, Action::DeletePlaylist],
            now,
        );
        assert_eq!(status_text(&app, now), READ_ONLY_OTHER);
        // `N` and `P` still work from someone else's playlist.
        press(&mut app, &[Action::NewPlaylist], now);
        assert_eq!(browser(&app).prompt(), Some(("New playlist", "")));
        press(&mut app, &[Action::InputCancel], now);
        // Inside it, nothing can change either.
        open_playlist(
            &mut app,
            playlist_songs(&["s1", "s2"], owned_by(Some("someone"), true)),
            now,
        );
        for action in [
            Action::RemoveFromPlaylist,
            Action::MoveEntryDown,
            Action::MoveEntryUp,
        ] {
            app.set_status("", now);
            press(&mut app, &[action, action], now);
            assert_eq!(status_text(&app, now), READ_ONLY_OTHER, "{action:?}");
        }
        assert!(!shown_title(&app).contains(MODIFIED));
        // The server's own read-only flag, and an unknown owner.
        press(&mut app, &[Action::Parent, Action::Down], now);
        open_playlist(
            &mut app,
            playlist_songs(&["s1", "s2"], owned_by(Some("u"), true)),
            now,
        );
        press(&mut app, &[Action::MoveEntryDown], now);
        assert_eq!(status_text(&app, now), READ_ONLY_FLAGGED);
        press(&mut app, &[Action::Parent, Action::Down], now);
        open_playlist(
            &mut app,
            playlist_songs(&["s1", "s2"], owned_by(None, false)),
            now,
        );
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        assert_eq!(status_text(&app, now), READ_ONLY_UNKNOWN);
        assert!(app.take_edit_requests().is_empty());
    }

    #[test]
    fn new_and_rename_prompts_emit_edits_and_an_empty_name_cancels() {
        let (mut app, now) = playlists_app(vec![own("p1", "Road Trip")]);
        press(&mut app, &[Action::NewPlaylist], now);
        assert!(app.input_open());
        // Prompt keys are text, not commands.
        type_text(&mut app, "  Night ", now);
        app.handle_paste("Drive\n\u{1b}");
        assert_eq!(
            browser(&app).prompt(),
            Some(("New playlist", "  Night Drive "))
        );
        press(&mut app, &[Action::InputSubmit], now);
        assert!(!app.input_open());
        assert_eq!(status_text(&app, now), "Creating Night Drive…");
        let create = one_edit(&mut app);
        assert_eq!(
            create.op,
            EditOp::Create {
                name: "Night Drive".into(),
                songs: Vec::new()
            }
        );
        // An empty name cancels.
        press(&mut app, &[Action::NewPlaylist, Action::InputSubmit], now);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "   ", now);
        press(&mut app, &[Action::InputSubmit], now);
        // Rename: prefilled; unchanged does nothing.
        press(&mut app, &[Action::RenamePlaylist], now);
        assert_eq!(
            browser(&app).prompt(),
            Some(("Rename playlist", "Road Trip"))
        );
        press(&mut app, &[Action::InputSubmit], now);
        assert_eq!(status_text(&app, now), "Name unchanged");
        press(&mut app, &[Action::RenamePlaylist], now);
        type_text(&mut app, "s", now);
        press(&mut app, &[Action::InputSubmit], now);
        answer_edit(
            &mut app,
            &create,
            EditResult::Done {
                created: Some("new".into()),
            },
        );
        assert_eq!(status_text(&app, now), "Created Night Drive");
        let rename = one_edit(&mut app);
        assert_eq!(
            rename.op,
            EditOp::Rename {
                playlist_id: "p1".into(),
                name: "Road Trips".into()
            }
        );
        assert!(app.take_edit_requests().is_empty(), "nothing else queued");
        // The Playlists list was reloaded after the create.
        let reload = one_request(&mut app);
        assert_eq!(reload.resource, Resource::Playlists);
        assert_eq!(current_load(&app), Load::Loading(reload.id));
        answer_edit(&mut app, &rename, EditResult::Done { created: None });
        assert_eq!(status_text(&app, now), "Renamed Road Trip to Road Trips");
    }

    #[test]
    fn delete_needs_a_second_press_on_the_same_playlist_soon() {
        let (mut app, now) = playlists_app(vec![own("p1", "One"), own("p2", "Two")]);
        press(&mut app, &[Action::DeletePlaylist], now);
        assert_eq!(status_text(&app, now), "Press D again to delete One");
        assert!(app.take_edit_requests().is_empty());
        // Anything in between disarms it, and so does moving to another.
        press(&mut app, &[Action::Down, Action::DeletePlaylist], now);
        assert_eq!(status_text(&app, now), "Press D again to delete Two");
        press(&mut app, &[Action::Up, Action::DeletePlaylist], now);
        assert_eq!(status_text(&app, now), "Press D again to delete One");
        // Too late.
        let later = now + CLEAR_CONFIRM + Duration::from_millis(1);
        app.handle_action(Action::DeletePlaylist, later);
        assert!(app.take_edit_requests().is_empty());
        assert_eq!(status_text(&app, later), "Press D again to delete One");
        app.handle_action(Action::DeletePlaylist, later);
        let delete = one_edit(&mut app);
        assert_eq!(
            delete.op,
            EditOp::Delete {
                playlist_id: "p1".into()
            }
        );
        assert_eq!(status_text(&app, later), "Deleting One…");
        answer_edit(&mut app, &delete, EditResult::Done { created: None });
        assert_eq!(status_text(&app, later), "Deleted One");
        assert_eq!(one_request(&mut app).resource, Resource::Playlists);
    }

    #[test]
    fn remove_needs_a_second_press_and_a_stale_playlist_is_reloaded() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2", "s1"]), now);
        press(&mut app, &[Action::Down, Action::Down], now);
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        assert_eq!(status_text(&app, now), "Press d again to remove Remote s1");
        assert!(app.take_edit_requests().is_empty());
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        let remove = one_edit(&mut app);
        assert_eq!(
            remove.op,
            EditOp::Remove {
                playlist_id: "p1".into(),
                index: 2,
                expected: vec!["s1".into(), "s2".into(), "s1".into()],
                hidden: 0,
            }
        );
        // The queue is not touched.
        assert!(app.player().queue().is_empty());
        answer_edit(&mut app, &remove, EditResult::Stale);
        assert_eq!(status_text(&app, now), STALE_PLAYLIST);
        // The shown playlist is fetched again, and so is the Playlists list
        // below it (counts may have changed).
        let reloads = requests(&mut app);
        let resources: Vec<&Resource> = reloads.iter().map(|r| &r.resource).collect();
        assert_eq!(
            resources,
            [&Resource::Playlist("p1".into()), &Resource::Playlists]
        );
        let reload = reloads[0].clone();
        respond(&mut app, &reload, Ok(own_songs(&["s1", "s3", "s2", "s1"])));
        assert_eq!(
            song_titles(&app),
            ["Remote s1", "Remote s3", "Remote s2", "Remote s1"]
        );
        // A successful removal reloads it too.
        press(
            &mut app,
            &[Action::RemoveFromPlaylist, Action::RemoveFromPlaylist],
            now,
        );
        let remove = one_edit(&mut app);
        answer_edit(&mut app, &remove, EditResult::Done { created: None });
        // The selection kept its row across the reload.
        assert_eq!(status_text(&app, now), "Removed Remote s2 from Mix");
        assert_eq!(
            requests(&mut app)[0].resource,
            Resource::Playlist("p1".into())
        );
    }

    #[test]
    fn index_edits_are_refused_on_a_playlist_with_unreadable_entries() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        let Content::PlaylistSongs(mut songs) = own_songs(&["s1", "s2"]) else {
            unreachable!()
        };
        songs.served = 3;
        open_playlist(&mut app, Content::PlaylistSongs(songs), now);
        for action in [Action::RemoveFromPlaylist, Action::MoveEntryDown] {
            press(&mut app, &[action], now);
            assert_eq!(status_text(&app, now), INCOMPLETE_PLAYLIST);
        }
        assert!(app.take_edit_requests().is_empty());
    }

    #[test]
    fn reordering_is_a_draft_until_saved_or_discarded() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2", "s3"]), now);
        press(&mut app, &[Action::MoveEntryUp], now);
        assert!(
            !shown_title(&app).contains(MODIFIED),
            "nothing above the first"
        );
        press(&mut app, &[Action::MoveEntryDown], now);
        assert_eq!(song_titles(&app), ["Remote s2", "Remote s1", "Remote s3"]);
        assert_eq!(current_selected(&app), 1, "the selection follows");
        assert_eq!(shown_title(&app), format!("Mix{MODIFIED}"));
        // Leaving or reloading is refused while it is unsaved.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(status_text(&app, now), UNSAVED_ORDER);
        press(&mut app, &[Action::Refresh], now);
        assert!(requests(&mut app).is_empty());
        assert_eq!(view_titles(&app), ["Playlists", "Mix"]);
        // Removing is refused too.
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        assert_eq!(status_text(&app, now), UNSAVED_ORDER);
        // Esc discards.
        press(&mut app, &[Action::Back], now);
        assert_eq!(status_text(&app, now), "New order discarded");
        assert_eq!(song_titles(&app), ["Remote s1", "Remote s2", "Remote s3"]);
        assert_eq!(shown_title(&app), "Mix");
        // Moving back to the start is no change.
        press(&mut app, &[Action::MoveEntryDown, Action::MoveEntryUp], now);
        assert_eq!(shown_title(&app), "Mix");
        press(&mut app, &[Action::SaveOrder], now);
        assert!(app.take_edit_requests().is_empty(), "nothing to save");
        // Move s1 to the end and save.
        press(&mut app, &[Action::Up], now);
        assert_eq!(current_selected(&app), 0);
        press(
            &mut app,
            &[
                Action::MoveEntryDown,
                Action::MoveEntryDown,
                Action::SaveOrder,
            ],
            now,
        );
        assert_eq!(status_text(&app, now), "Saving the new order of Mix…");
        let replace = one_edit(&mut app);
        assert_eq!(
            replace.op,
            EditOp::Replace {
                playlist_id: "p1".into(),
                expected: vec!["s1".into(), "s2".into(), "s3".into()],
                songs: vec!["s2".into(), "s3".into(), "s1".into()],
                hidden: 0,
            }
        );
        press(&mut app, &[Action::MoveEntryUp], now);
        assert_eq!(status_text(&app, now), "Saving the new order…");
        // A failure keeps the draft on screen, unreloaded, so `w` retries.
        answer_edit(&mut app, &replace, EditResult::Failed(server_error("busy")));
        assert_eq!(
            status_text(&app, now),
            "Could not save the order of Mix: server error 0: busy; w retries, Esc discards"
        );
        assert!(
            requests(&mut app).is_empty(),
            "nothing changed, nothing reloaded"
        );
        assert_eq!(current_load(&app), Load::Ready);
        assert_eq!(shown_title(&app), format!("Mix{MODIFIED}"));
        assert_eq!(song_titles(&app), ["Remote s2", "Remote s3", "Remote s1"]);
        // Too long for one request on this server: no retry offered.
        press(&mut app, &[Action::SaveOrder], now);
        let replace = one_edit(&mut app);
        answer_edit(
            &mut app,
            &replace,
            EditResult::Failed(navidrome::Error::RequestTooLarge),
        );
        assert_eq!(
            status_text(&app, now),
            "Could not save the order of Mix: too many songs to reorder on this server; Esc discards"
        );
        assert_eq!(shown_title(&app), format!("Mix{MODIFIED}"));
        press(&mut app, &[Action::SaveOrder], now);
        let replace = one_edit(&mut app);
        answer_edit(&mut app, &replace, EditResult::Done { created: None });
        assert_eq!(status_text(&app, now), "Saved the new order of Mix");
        let reload = requests(&mut app).remove(0);
        assert_eq!(reload.resource, Resource::Playlist("p1".into()));
        respond(&mut app, &reload, Ok(own_songs(&["s2", "s3", "s1"])));
        assert_eq!(shown_title(&app), "Mix");
        assert_eq!(song_titles(&app), ["Remote s2", "Remote s3", "Remote s1"]);
        press(&mut app, &[Action::Parent], now);
        assert_eq!(view_titles(&app), ["Playlists"]);
    }

    #[test]
    fn a_stale_reorder_is_dropped_and_the_playlist_reloaded() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2"]), now);
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        let replace = one_edit(&mut app);
        answer_edit(&mut app, &replace, EditResult::Stale);
        assert_eq!(status_text(&app, now), STALE_PLAYLIST);
        let reload = requests(&mut app).remove(0);
        assert_eq!(reload.resource, Resource::Playlist("p1".into()));
        respond(&mut app, &reload, Ok(own_songs(&["s9", "s1", "s2"])));
        assert_eq!(shown_title(&app), "Mix");
        assert_eq!(song_titles(&app), ["Remote s9", "Remote s1", "Remote s2"]);
    }

    #[test]
    fn the_picker_lists_new_playlist_then_own_playlists_a_to_z() {
        let (mut app, now) = nav_app_with_artists(&["a1"]);
        press(&mut app, &[Action::AddToPlaylist], now);
        // The Playlists list is fetched for it.
        let lists = one_request(&mut app);
        assert_eq!(lists.resource, Resource::Playlists);
        let picker = browser(&app).picker().unwrap();
        assert_eq!(picker.label(), "Artist a1");
        assert_eq!(picker.load(), &Load::Loading(lists.id));
        respond(
            &mut app,
            &lists,
            Ok(Content::Playlists(vec![
                own("p1", "beta"),
                others("p2", "Alpha (theirs)"),
                own("p3", "Alpha"),
                Playlist {
                    access: owned_by(Some("u"), true),
                    ..own("p4", "Smart")
                },
            ])),
        );
        let picker = browser(&app).picker().unwrap();
        let names: Vec<&str> = picker.choices().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta"]);
        assert_eq!(picker.selected(), 0);
        // Keys move in the picker; the view behind does not.
        press(&mut app, &[Action::Down, Action::Down, Action::Down], now);
        assert_eq!(browser(&app).picker().unwrap().selected(), 2);
        press(&mut app, &[Action::Up], now);
        assert_eq!(current_selected(&app), 0);
        // Esc closes; `P` again uses the cached list.
        press(&mut app, &[Action::Back], now);
        assert!(browser(&app).picker().is_none());
        assert_eq!(view_titles(&app), ["Artists"]);
        press(&mut app, &[Action::AddToPlaylist], now);
        assert!(requests(&mut app).is_empty());
        assert_eq!(browser(&app).picker().unwrap().load(), &Load::Ready);
        // "New playlist…": a name, then the artist's songs are fetched.
        press(&mut app, &[Action::Activate], now);
        assert_eq!(browser(&app).prompt(), Some(("New playlist", "")));
        type_text(&mut app, "Fresh", now);
        press(&mut app, &[Action::InputSubmit], now);
        assert_eq!(status_text(&app, now), "Fetching Artist a1…");
        let artist = one_request(&mut app);
        respond(&mut app, &artist, Ok(Content::Albums(vec![album("x")])));
        let songs_request = one_request(&mut app);
        respond(&mut app, &songs_request, Ok(songs(&["s1", "s2"])));
        assert_eq!(
            status_text(&app, now),
            "Creating Fresh with 2 songs from Artist a1…"
        );
        assert_eq!(
            one_edit(&mut app).op,
            EditOp::Create {
                name: "Fresh".into(),
                songs: vec!["s1".into(), "s2".into()]
            }
        );
        // Nothing reached the queue.
        assert!(app.player().queue().is_empty());
    }

    #[test]
    fn adding_a_playlist_to_another_reports_a_partial_failure() {
        let (mut app, now) = playlists_app(vec![own("p1", "b list"), own("p2", "A list")]);
        press(
            &mut app,
            &[Action::AddToPlaylist, Action::Down, Action::Activate],
            now,
        );
        assert!(browser(&app).picker().is_none());
        let source = one_request(&mut app);
        assert_eq!(source.resource, Resource::Playlist("p1".into()));
        respond(&mut app, &source, Ok(own_songs(&["s1", "s2", "s1"])));
        assert_eq!(
            status_text(&app, now),
            "Adding 3 songs from b list to A list…"
        );
        let add = one_edit(&mut app);
        assert_eq!(
            add.op,
            EditOp::Add {
                playlist_id: "p2".into(),
                songs: vec!["s1".into(), "s2".into(), "s1".into()]
            }
        );
        answer_edit(
            &mut app,
            &add,
            EditResult::Partial {
                created: None,
                added: 1,
                error: server_error("disk full"),
            },
        );
        assert_eq!(
            status_text(&app, now),
            "Added 1 of 3 songs to A list; could not add the rest: server error 0: disk full"
        );
        // The shown Playlists list is reloaded.
        assert_eq!(one_request(&mut app).resource, Resource::Playlists);
    }

    #[test]
    fn local_tracks_cannot_go_to_a_navidrome_playlist() {
        let (mut app, now) = app_with(&["a/one.flac"]);
        press(&mut app, &[Action::AddToPlaylist], now);
        assert_eq!(status_text(&app, now), LOCAL_TO_PLAYLIST);
    }

    #[test]
    fn the_queue_is_saved_as_a_playlist_of_its_navidrome_songs() {
        let (mut app, now) = nav_home();
        app.player.enqueue(remote("s1"));
        app.player.enqueue(track("local.flac"));
        app.player.enqueue(remote("s2"));
        app.player.enqueue(remote("s1"));
        // Only in the queue panel.
        press(&mut app, &[Action::SaveQueueAsPlaylist], now);
        assert!(browser(&app).prompt().is_none());
        press(
            &mut app,
            &[Action::SwitchFocus, Action::SaveQueueAsPlaylist],
            now,
        );
        let (title, text) = browser(&app).prompt().unwrap();
        assert_eq!(title, "Save queue as playlist");
        assert!(
            text.starts_with("Queue 20") && text.len() == "Queue 2026-11-01".len(),
            "{text}"
        );
        press(&mut app, &[Action::InputSubmit], now);
        assert!(
            status_text(&app, now).starts_with("Saving 3 songs as Queue 20"),
            "{}",
            status_text(&app, now)
        );
        assert!(status_text(&app, now).ends_with("(1 local track left out)…"));
        let EditOp::Create { songs, .. } = one_edit(&mut app).op else {
            panic!()
        };
        assert_eq!(songs, ["s1", "s2", "s1"]);
        assert_eq!(app.player().queue().len(), 4, "the queue is unchanged");
        // Without Navidrome songs, or without Navidrome.
        let (mut app, now) = queued_app(&["a", "b"]);
        press(&mut app, &[Action::SaveQueueAsPlaylist], now);
        assert_eq!(status_text(&app, now), NAVIDROME_NOT_CONFIGURED);
        let (mut app, now) = nav_home();
        app.player.enqueue(track("local.flac"));
        press(
            &mut app,
            &[Action::SwitchFocus, Action::SaveQueueAsPlaylist],
            now,
        );
        assert_eq!(status_text(&app, now), NO_REMOTE_IN_QUEUE);
    }

    #[test]
    fn the_default_queue_name_is_the_local_date() {
        // The UTC fallback.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(951_782_400 / 86_400), (2000, 2, 29));
        assert_eq!(civil_from_days(1_793_491_199 / 86_400), (2026, 10, 31));
        assert_eq!(civil_from_days(1_793_491_200 / 86_400), (2026, 11, 1));
        // The local date is within a day of the UTC date (time zones span
        // UTC-12 to UTC+14) and is what the name uses.
        for secs in [0u64, 951_782_400, 1_793_491_200, 1_793_534_400] {
            let (y, m, d) = local_date(secs).expect("localtime_r");
            let local = format!("Queue {y:04}-{m:02}-{d:02}");
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
            assert_eq!(queue_playlist_name(at), local);
            let near: Vec<String> = [secs.saturating_sub(86_400), secs, secs + 86_400]
                .iter()
                .map(|s| {
                    let (y, m, d) = civil_from_days(s / 86_400);
                    format!("Queue {y:04}-{m:02}-{d:02}")
                })
                .collect();
            assert!(near.contains(&local), "{local} vs {near:?}");
        }
    }

    #[test]
    fn edits_run_one_at_a_time_and_one_in_flight_at_a_swap_is_dropped() {
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        app.set_settings_context(settings_context(None, coda_owned()));
        for name in ["A", "B"] {
            press(&mut app, &[Action::NewPlaylist], now);
            type_text(&mut app, name, now);
            press(&mut app, &[Action::InputSubmit], now);
        }
        let first = one_edit(&mut app);
        assert_eq!(first.generation, Generation(0));
        assert!(app.take_edit_requests().is_empty(), "B waits for A");
        assert_eq!(browser(&app).pending_edits(), 2);
        connect_in_app(&mut app, other_client(), true, now);
        let swapped = status_text(&app, now);
        answer_edit(&mut app, &first, EditResult::Done { created: None });
        assert_eq!(status_text(&app, now), swapped, "the old result is dropped");
        assert!(
            app.take_edit_requests().is_empty(),
            "B went with the old browser"
        );
        assert_eq!(browser(&app).pending_edits(), 0);
        assert!(requests(&mut app).is_empty(), "nothing reloaded");
    }

    #[test]
    fn a_reload_after_an_edit_supersedes_an_older_fetch() {
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        press(&mut app, &[Action::Refresh], now);
        let older = one_request(&mut app);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Two", now);
        press(&mut app, &[Action::InputSubmit], now);
        let create = one_edit(&mut app);
        answer_edit(
            &mut app,
            &create,
            EditResult::Done {
                created: Some("p2".into()),
            },
        );
        let newer = one_request(&mut app);
        assert_eq!(newer.resource, Resource::Playlists);
        assert_ne!(newer.id, older.id);
        // The older answer may predate the change: neither cached nor shown.
        respond(
            &mut app,
            &older,
            Ok(Content::Playlists(vec![own("p1", "One")])),
        );
        assert_eq!(current_load(&app), Load::Loading(newer.id));
        assert!(browser(&app).cached(&Resource::Playlists).is_none());
        respond(
            &mut app,
            &newer,
            Ok(Content::Playlists(vec![own("p1", "One"), own("p2", "Two")])),
        );
        assert_eq!(current_load(&app), Load::Ready);
        assert_eq!(browser(&app).current_content().unwrap().len(), 2);
    }

    // ----- review follow-up: drafts, picker, quitting, serialisation -----

    use crate::navidrome::browse::{LAST_SONG, ORDER_DISCARDED, SAVING_ORDER, WAITING_FOR_EDIT};

    #[test]
    fn esc_during_a_save_is_refused_and_only_that_save_ends_the_draft() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2", "s3"]), now);
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        let save = one_edit(&mut app);
        // Esc while it runs: refused, the draft stays.
        press(&mut app, &[Action::Back], now);
        assert_eq!(status_text(&app, now), SAVING_ORDER);
        assert_eq!(shown_title(&app), format!("Mix{MODIFIED}"));
        // `J` after the refused Esc is refused too; no new draft.
        press(&mut app, &[Action::MoveEntryDown], now);
        assert_eq!(status_text(&app, now), SAVING_ORDER);
        assert_eq!(song_titles(&app), ["Remote s2", "Remote s1", "Remote s3"]);
        answer_edit(&mut app, &save, EditResult::Done { created: None });
        assert_eq!(status_text(&app, now), "Saved the new order of Mix");
        assert_eq!(shown_title(&app), "Mix");
        // After the save, a new draft works and Esc discards it.
        let reload = requests(&mut app).remove(0);
        respond(&mut app, &reload, Ok(own_songs(&["s2", "s1", "s3"])));
        press(&mut app, &[Action::MoveEntryDown, Action::Back], now);
        assert_eq!(status_text(&app, now), ORDER_DISCARDED);
        assert_eq!(shown_title(&app), "Mix");
    }

    #[test]
    fn a_stale_answer_for_another_save_does_not_end_a_new_draft() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2", "s3"]), now);
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        let save = one_edit(&mut app);
        // An answer with another id (a forged or old one) leaves it alone.
        answer_edit(
            &mut app,
            &EditRequest {
                id: crate::navidrome::edit::EditId(save.id.0 + 7),
                ..save.clone()
            },
            EditResult::Done { created: None },
        );
        assert_eq!(shown_title(&app), format!("Mix{MODIFIED}"));
        press(&mut app, &[Action::Back], now);
        assert_eq!(status_text(&app, now), SAVING_ORDER);
    }

    #[test]
    fn the_picker_keeps_its_choices_and_selection_across_a_reload() {
        let (mut app, now) = playlists_app(vec![own("p1", "One"), own("p2", "Two")]);
        // An edit is running when the picker opens.
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Zed", now);
        press(&mut app, &[Action::InputSubmit], now);
        let create = one_edit(&mut app);
        press(
            &mut app,
            &[Action::AddToPlaylist, Action::Down, Action::Down],
            now,
        );
        assert_eq!(browser(&app).picker().unwrap().selected(), 2, "Two");
        answer_edit(
            &mut app,
            &create,
            EditResult::Done {
                created: Some("p3".into()),
            },
        );
        let reload = one_request(&mut app);
        assert_eq!(reload.resource, Resource::Playlists);
        let picker = browser(&app).picker().unwrap();
        assert_eq!(picker.load(), &Load::Loading(reload.id));
        let names: Vec<&str> = picker.choices().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["One", "Two"], "old choices stay while loading");
        // The new list puts another playlist first; the selection follows Two.
        respond(
            &mut app,
            &reload,
            Ok(Content::Playlists(vec![
                own("p1", "One"),
                own("p2", "Two"),
                own("p3", "Ab"),
            ])),
        );
        let picker = browser(&app).picker().unwrap();
        assert_eq!(picker.selected(), 3);
        assert_eq!(picker.choices()[2].name, "Two");
        press(&mut app, &[Action::Back], now);
    }

    #[test]
    fn enter_in_a_reloading_picker_adds_to_the_playlist_shown() {
        let (mut app, now) = playlists_app(vec![own("p1", "One"), own("p2", "Two")]);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Zed", now);
        press(&mut app, &[Action::InputSubmit], now);
        let create = one_edit(&mut app);
        press(&mut app, &[Action::AddToPlaylist, Action::Down], now);
        answer_edit(&mut app, &create, EditResult::Done { created: None });
        let reload = one_request(&mut app);
        // Still loading: Enter acts on the row shown ("One").
        press(&mut app, &[Action::Activate], now);
        assert!(browser(&app).picker().is_none());
        let songs = one_request(&mut app);
        assert_eq!(songs.resource, Resource::Playlist("p1".into()));
        respond(&mut app, &songs, Ok(own_songs(&["s1"])));
        let add = one_edit(&mut app);
        assert_eq!(
            add.op,
            EditOp::Add {
                playlist_id: "p1".into(),
                songs: vec!["s1".into()]
            }
        );
        // A selected playlist that is gone after a reload falls back to
        // "New playlist…".
        respond(
            &mut app,
            &reload,
            Ok(Content::Playlists(vec![own("p1", "One"), own("p2", "Two")])),
        );
        press(
            &mut app,
            &[Action::AddToPlaylist, Action::Down, Action::Down],
            now,
        );
        assert_eq!(browser(&app).picker().unwrap().selected(), 2);
        answer_edit(&mut app, &add, EditResult::Done { created: None });
        let lists = requests(&mut app)
            .into_iter()
            .find(|r| r.resource == Resource::Playlists)
            .unwrap();
        respond(
            &mut app,
            &lists,
            Ok(Content::Playlists(vec![own("p1", "One")])),
        );
        assert_eq!(browser(&app).picker().unwrap().selected(), 0);
    }

    #[test]
    fn quitting_with_a_pending_change_warns_first() {
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Zed", now);
        press(&mut app, &[Action::InputSubmit], now);
        press(&mut app, &[Action::Quit], now);
        assert!(!app.should_quit());
        assert_eq!(status_text(&app, now), SAVING_PLAYLIST);
        press(&mut app, &[Action::Quit], now);
        assert!(app.should_quit());
        // Once the change answered, quitting is immediate again.
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Zed", now);
        press(&mut app, &[Action::InputSubmit, Action::Quit], now);
        let create = one_edit(&mut app);
        answer_edit(&mut app, &create, EditResult::Done { created: None });
        press(&mut app, &[Action::Quit], now);
        assert!(app.should_quit());
        // From the help overlay too.
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "Zed", now);
        press(
            &mut app,
            &[Action::InputSubmit, Action::ToggleHelp, Action::Quit],
            now,
        );
        assert!(!app.should_quit());
        assert_eq!(app.browser().unwrap().queued_edits(), 1);
    }

    #[test]
    fn index_edits_wait_for_a_pending_change_of_the_same_playlist() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2", "s3"]), now);
        press(
            &mut app,
            &[Action::RemoveFromPlaylist, Action::RemoveFromPlaylist],
            now,
        );
        assert_eq!(app.browser().unwrap().pending_edits(), 1);
        for action in [Action::RemoveFromPlaylist, Action::MoveEntryDown] {
            press(&mut app, &[action], now);
            assert_eq!(status_text(&app, now), WAITING_FOR_EDIT, "{action:?}");
        }
        assert!(!shown_title(&app).contains(MODIFIED));
        assert_eq!(app.take_edit_requests().len(), 1, "only the first");
    }

    #[test]
    fn the_last_song_cannot_be_removed() {
        let (mut app, now) = playlists_app(vec![own("p1", "Solo")]);
        open_playlist(&mut app, own_songs(&["s1"]), now);
        press(
            &mut app,
            &[Action::RemoveFromPlaylist, Action::RemoveFromPlaylist],
            now,
        );
        assert_eq!(status_text(&app, now), LAST_SONG);
        assert!(app.take_edit_requests().is_empty());
    }

    #[test]
    fn search_cannot_open_over_an_unsaved_order() {
        let (mut app, now) = playlists_app(vec![own("p1", "Mix")]);
        open_playlist(&mut app, own_songs(&["s1", "s2"]), now);
        press(&mut app, &[Action::MoveEntryDown, Action::OpenSearch], now);
        assert!(!app.input_open());
        assert_eq!(status_text(&app, now), UNSAVED_ORDER);
    }

    #[test]
    fn an_untouched_rename_sends_nothing_even_for_a_cut_name() {
        let long = "n".repeat(300);
        let (mut app, now) = playlists_app(vec![own("p1", &long)]);
        press(&mut app, &[Action::RenamePlaylist], now);
        assert_eq!(
            browser(&app).prompt().unwrap().1.chars().count(),
            MAX_QUERY_CHARS
        );
        press(&mut app, &[Action::InputSubmit], now);
        assert_eq!(status_text(&app, now), "Name unchanged");
        assert!(app.take_edit_requests().is_empty());
    }

    #[test]
    fn an_edit_supersedes_a_fetch_in_flight_for_a_list_no_view_shows() {
        let (mut app, now) = playlists_app(vec![own("p1", "One")]);
        press(&mut app, &[Action::Refresh], now);
        let older = one_request(&mut app);
        press(&mut app, &[Action::Parent], now);
        assert!(view_titles(&app).is_empty(), "on Home");
        app.player.enqueue(remote("s1"));
        press(
            &mut app,
            &[Action::SwitchFocus, Action::SaveQueueAsPlaylist],
            now,
        );
        press(&mut app, &[Action::InputSubmit], now);
        let create = one_edit(&mut app);
        answer_edit(
            &mut app,
            &create,
            EditResult::Done {
                created: Some("p2".into()),
            },
        );
        let newer = one_request(&mut app);
        assert_eq!(newer.resource, Resource::Playlists);
        respond(
            &mut app,
            &older,
            Ok(Content::Playlists(vec![own("p1", "One")])),
        );
        assert!(
            browser(&app).cached(&Resource::Playlists).is_none(),
            "predates the edit"
        );
        respond(
            &mut app,
            &newer,
            Ok(Content::Playlists(vec![
                own("p1", "One"),
                own("p2", "Queue"),
            ])),
        );
        let Some(Content::Playlists(lists)) = browser(&app).cached(&Resource::Playlists) else {
            panic!()
        };
        assert_eq!(lists.len(), 2);
    }

    // ----- the loop's edit runner ----------------------------------------

    use wiremock::matchers::path as at_path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn subsonic_ok(body: serde_json::Value) -> ResponseTemplate {
        let mut response = serde_json::json!({ "status": "ok", "version": "1.16.1" });
        if let (Some(target), serde_json::Value::Object(extra)) = (response.as_object_mut(), body) {
            target.extend(extra);
        }
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "subsonic-response": response }))
    }

    async fn edit_server(delete_delay: Duration) -> MockServer {
        let server = MockServer::start().await;
        let mount = |endpoint: &str, response: ResponseTemplate| {
            Mock::given(at_path(format!("/nd/rest/{endpoint}"))).respond_with(response)
        };
        mount(
            "getOpenSubsonicExtensions",
            subsonic_ok(serde_json::json!({ "openSubsonicExtensions": [{ "name": "formPost" }] })),
        )
        .mount(&server)
        .await;
        mount(
            "deletePlaylist",
            subsonic_ok(serde_json::json!({})).set_delay(delete_delay),
        )
        .mount(&server)
        .await;
        mount(
            "createPlaylist",
            subsonic_ok(serde_json::json!({ "playlist": { "id": "p9" } })),
        )
        .mount(&server)
        .await;
        server
    }

    fn client_for(server: &MockServer) -> navidrome::Client {
        navidrome::Client::new(
            navidrome::client::parse_base_url(&format!("{}/nd", server.uri())).unwrap(),
            "u".into(),
            navidrome::Password::new("p".into()),
        )
        .unwrap()
    }

    /// Navidrome only, against `client`, in the Playlists list (answered
    /// by hand).
    fn runner_app(client: navidrome::Client) -> (App, Instant) {
        let mut app = App::with_sources(None, Some(client));
        let now = Instant::now();
        press(&mut app, &[Action::Activate], now);
        let request = one_request(&mut app);
        respond(
            &mut app,
            &request,
            Ok(Content::Playlists(vec![own("p1", "One")])),
        );
        (app, now)
    }

    fn endpoints(requests: &[wiremock::Request]) -> Vec<String> {
        requests
            .iter()
            .map(|r| r.url.path().rsplit('/').next().unwrap().to_owned())
            .filter(|e| e != "getOpenSubsonicExtensions")
            .collect()
    }

    #[tokio::test]
    async fn an_edit_running_across_a_same_account_swap_finishes_before_the_next_starts() {
        let server = edit_server(Duration::from_millis(400)).await;
        let client = client_for(&server);
        let (mut app, now) = runner_app(client.clone());
        app.set_settings_context(settings_context(None, coda_owned()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runner = EditRunner::default();
        press(
            &mut app,
            &[Action::DeletePlaylist, Action::DeletePlaylist],
            now,
        );
        runner.pump(&mut app, &tx);
        assert!(runner.running.is_some());
        // The same account is saved again: a new generation and browser.
        connect_in_app(&mut app, client, true, now);
        app.handle_settings_input(Input::Esc, now);
        press(&mut app, &[Action::Activate], now);
        let request = one_request(&mut app);
        assert_eq!(request.generation, Generation(1));
        respond(
            &mut app,
            &request,
            Ok(Content::Playlists(vec![own("p1", "One")])),
        );
        press(&mut app, &[Action::NewPlaylist], now);
        type_text(&mut app, "New", now);
        press(&mut app, &[Action::InputSubmit], now);
        // The old edit still runs: the new one waits, queued.
        runner.pump(&mut app, &tx);
        assert_eq!(app.browser().unwrap().queued_edits(), 1);
        let first = rx.recv().await.unwrap();
        assert_eq!(first.generation, Generation(0));
        assert_eq!(
            endpoints(&server.received_requests().await.unwrap()),
            ["deletePlaylist"]
        );
        runner.answered(&mut app, &first);
        app.handle_edit_response(first, Instant::now());
        runner.pump(&mut app, &tx);
        assert_eq!(app.browser().unwrap().queued_edits(), 0);
        let second = rx.recv().await.unwrap();
        assert_eq!(second.generation, Generation(1));
        runner.answered(&mut app, &second);
        app.handle_edit_response(second, Instant::now());
        assert_eq!(status_text(&app, Instant::now()), "Created New");
        assert_eq!(
            endpoints(&server.received_requests().await.unwrap()),
            ["deletePlaylist", "createPlaylist"]
        );
    }

    #[tokio::test]
    async fn the_exit_waits_for_the_running_edit_but_only_so_long() {
        // A quick edit is waited for.
        let server = edit_server(Duration::from_millis(200)).await;
        let (mut app, now) = runner_app(client_for(&server));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runner = EditRunner::default();
        press(
            &mut app,
            &[Action::DeletePlaylist, Action::DeletePlaylist],
            now,
        );
        runner.pump(&mut app, &tx);
        let started = Instant::now();
        runner.finish(Duration::from_secs(5)).await;
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(matches!(
            rx.try_recv().unwrap().result,
            crate::navidrome::edit::EditResult::Done { .. }
        ));
        // A hung one is not.
        let server = edit_server(Duration::from_secs(10)).await;
        let (mut app, now) = runner_app(client_for(&server));
        let mut runner = EditRunner::default();
        press(
            &mut app,
            &[Action::DeletePlaylist, Action::DeletePlaylist],
            now,
        );
        runner.pump(&mut app, &tx);
        let started = Instant::now();
        runner.finish(Duration::from_millis(100)).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        // Nothing running: returns at once.
        EditRunner::default().finish(Duration::from_secs(5)).await;
    }

    // ----- hidden entries and the old account's edit at quit -------------

    /// Own songs whose stored count is `count` (hidden entries beyond).
    fn counted_songs(ids: &[&str], count: Option<u32>) -> Content {
        let Content::PlaylistSongs(mut songs) = own_songs(ids) else {
            unreachable!()
        };
        songs.song_count = count;
        Content::PlaylistSongs(songs)
    }

    #[test]
    fn removing_from_a_playlist_with_hidden_entries_warns_first() {
        let (mut app, now) = playlists_app(vec![own("p1", "Gaps")]);
        open_playlist(&mut app, counted_songs(&["s1", "s2"], Some(5)), now);
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        assert_eq!(
            status_text(&app, now),
            "Press d again to remove Remote s1: this also removes 3 entries Navidrome hides \
             (missing files or other libraries)"
        );
        assert!(app.take_edit_requests().is_empty());
        // Any other key cancels.
        press(
            &mut app,
            &[Action::CycleRepeat, Action::RemoveFromPlaylist],
            now,
        );
        assert!(app.take_edit_requests().is_empty());
        press(&mut app, &[Action::RemoveFromPlaylist], now);
        assert_eq!(
            one_edit(&mut app).op,
            EditOp::Remove {
                playlist_id: "p1".into(),
                index: 0,
                expected: vec!["s1".into(), "s2".into()],
                hidden: 3,
            }
        );
    }

    #[test]
    fn saving_an_order_over_hidden_entries_needs_a_second_w() {
        let (mut app, now) = playlists_app(vec![own("p1", "Gaps")]);
        open_playlist(&mut app, counted_songs(&["s1", "s2"], Some(3)), now);
        // Reordering itself is allowed; the warning comes at save.
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        assert_eq!(
            status_text(&app, now),
            "Press w again to save: this also removes 1 entry Navidrome hides (missing files or \
             other libraries)"
        );
        assert!(app.take_edit_requests().is_empty());
        // Another key cancels: the next `w` warns again.
        press(&mut app, &[Action::CycleRepeat, Action::SaveOrder], now);
        assert!(app.take_edit_requests().is_empty());
        press(&mut app, &[Action::SaveOrder], now);
        assert_eq!(status_text(&app, now), "Saving the new order of Gaps…");
        assert_eq!(
            one_edit(&mut app).op,
            EditOp::Replace {
                playlist_id: "p1".into(),
                expected: vec!["s1".into(), "s2".into()],
                songs: vec!["s2".into(), "s1".into()],
                hidden: 1,
            }
        );
    }

    #[test]
    fn a_suspect_count_warns_and_no_count_saves_at_once() {
        let (mut app, now) = playlists_app(vec![own("p1", "Odd"), own("p2", "Plain")]);
        open_playlist(&mut app, counted_songs(&["s1", "s2"], Some(1)), now);
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        assert!(
            status_text(&app, now)
                .starts_with("Press w again to save: this may also remove entries Navidrome hides"),
            "{}",
            status_text(&app, now)
        );
        press(&mut app, &[Action::SaveOrder], now);
        let EditOp::Replace { hidden, .. } = one_edit(&mut app).op else {
            panic!()
        };
        assert_eq!(hidden, usize::MAX);
        // A server that sends no songCount: no warning, as before.
        let (mut app, now) = playlists_app(vec![own("p1", "Plain")]);
        open_playlist(&mut app, counted_songs(&["s1", "s2"], None), now);
        press(&mut app, &[Action::MoveEntryDown, Action::SaveOrder], now);
        assert_eq!(status_text(&app, now), "Saving the new order of Plain…");
        let EditOp::Replace { hidden, .. } = one_edit(&mut app).op else {
            panic!()
        };
        assert_eq!(hidden, 0);
    }

    #[tokio::test]
    async fn quit_warns_while_an_edit_for_the_old_account_still_runs() {
        let server = edit_server(Duration::from_millis(300)).await;
        let client = client_for(&server);
        let (mut app, now) = runner_app(client.clone());
        app.set_settings_context(settings_context(None, coda_owned()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runner = EditRunner::default();
        press(
            &mut app,
            &[Action::DeletePlaylist, Action::DeletePlaylist],
            now,
        );
        runner.pump(&mut app, &tx);
        connect_in_app(&mut app, client, true, now);
        app.handle_settings_input(Input::Esc, now);
        assert_eq!(app.browser().unwrap().pending_edits(), 0, "a fresh browser");
        press(&mut app, &[Action::Quit], now);
        assert!(!app.should_quit());
        assert_eq!(status_text(&app, now), SAVING_PLAYLIST);
        let response = rx.recv().await.unwrap();
        runner.answered(&mut app, &response);
        app.handle_edit_response(response, Instant::now());
        // Done: the warning is disarmed and quitting is immediate.
        let (mut fresh, now) = (app, Instant::now());
        fresh.handle_action(Action::Quit, now);
        assert!(fresh.should_quit());
    }
}
