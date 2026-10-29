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
//! [`run`] owns the terminal lifecycle: it enters the TUI, installs the
//! panic hook, starts the scan, runs the loop, and on every exit shuts the
//! playback backend down (bounded) before restoring the terminal. Only this
//! module and `ui/` use Ratatui or Crossterm.

use std::io;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::library::scan::{self, ScanEvent};
use crate::library::tree::{DirNode, LibraryTree};
use crate::model::Track;
use crate::navidrome;
use crate::playback::{PlaybackCommand, PlaybackEvent, PlaybackHandle};
use crate::player::{Player, PlayerCommand};
use crate::resolve;
use crate::ui::{keymap, render};

/// Redraw interval when nothing else changes (progress, status expiry).
pub const TICK: Duration = Duration::from_millis(250);
/// How long a transient status message stays visible.
pub const STATUS_TTL: Duration = Duration::from_secs(5);
/// Upper bound on waiting for the playback backend at quit.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Seek step for the arrow keys, in seconds.
pub const SEEK_STEP: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Library,
    Queue,
}

/// A user intent, already resolved from a key by [`keymap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    /// Enter: open a directory, play a library track now, or play the
    /// selected queue entry.
    Activate,
    /// Library: go to the parent directory.
    Parent,
    /// Library: enqueue the track, or all tracks below the directory.
    Enqueue,
    /// Queue: remove the selected entry.
    Remove,
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
    Quit,
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

/// One row of the library panel: sub-directories first, then tracks.
#[derive(Debug, Clone, Copy)]
pub enum LibraryRow<'a> {
    Dir(&'a DirNode),
    Track(&'a Track),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
    pub expires_at: Instant,
}

/// Identity of a library row, used to keep the selection on the same item
/// while scan batches insert rows around it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Dir(String),
    Track(PathBuf),
}

#[derive(Debug)]
pub struct App {
    player: Player,
    tree: LibraryTree,
    /// Directory names from the library root to the open directory. The
    /// tree only grows, so this path stays valid.
    library_path: Vec<String>,
    library_selected: usize,
    queue_selected: usize,
    focus: Focus,
    help_open: bool,
    scan: ScanState,
    status: Option<StatusMessage>,
    quit: bool,
    /// Signs stream URLs for remote tracks; `None` if not configured.
    navidrome: Option<navidrome::Client>,
}

impl App {
    /// An app browsing `music_dir`, without Navidrome.
    pub fn new(music_dir: PathBuf) -> Self {
        Self::with_sources(Some(music_dir), None)
    }

    /// An app with an optional local library and an optional Navidrome
    /// client. Without a music directory the library stays empty and no
    /// scan is expected.
    pub fn with_sources(music_dir: Option<PathBuf>, navidrome: Option<navidrome::Client>) -> Self {
        let scan = match music_dir {
            Some(_) => ScanState::Scanning { tracks: 0 },
            None => ScanState::NoLibrary,
        };
        Self {
            player: Player::new(),
            tree: LibraryTree::new(music_dir.unwrap_or_default()),
            library_path: Vec::new(),
            library_selected: 0,
            queue_selected: 0,
            focus: Focus::Library,
            help_open: false,
            scan,
            status: None,
            quit: false,
            navidrome,
        }
    }

    // ----- read access for rendering -----------------------------------

    pub fn player(&self) -> &Player {
        &self.player
    }

    pub fn tree(&self) -> &LibraryTree {
        &self.tree
    }

    pub fn music_dir(&self) -> &Path {
        self.tree.root_path()
    }

    pub fn library_path(&self) -> &[String] {
        &self.library_path
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    pub fn scan_state(&self) -> ScanState {
        self.scan
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The transient message, if it has not expired at `now`.
    pub fn status(&self, now: Instant) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|s| now < s.expires_at)
            .map(|s| s.text.as_str())
    }

    /// The open library directory.
    pub fn library_dir(&self) -> &DirNode {
        let path: Vec<&str> = self.library_path.iter().map(String::as_str).collect();
        // The tree never shrinks, so the path always resolves; fall back to
        // the root defensively.
        self.tree.dir(&path).unwrap_or_else(|| self.tree.root())
    }

    pub fn library_rows(&self) -> Vec<LibraryRow<'_>> {
        let dir = self.library_dir();
        dir.dirs()
            .iter()
            .map(LibraryRow::Dir)
            .chain(dir.tracks().iter().map(LibraryRow::Track))
            .collect()
    }

    /// Number of rows in the open directory.
    pub fn library_row_count(&self) -> usize {
        self.library_len()
    }

    /// At most `len` rows starting at row `start`, without building the rest.
    pub fn library_rows_window(&self, start: usize, len: usize) -> Vec<LibraryRow<'_>> {
        let dir = self.library_dir();
        dir.dirs()
            .iter()
            .map(LibraryRow::Dir)
            .chain(dir.tracks().iter().map(LibraryRow::Track))
            .skip(start)
            .take(len)
            .collect()
    }

    fn library_len(&self) -> usize {
        let dir = self.library_dir();
        dir.dirs().len() + dir.tracks().len()
    }

    /// Selected library row index; always in range when there are rows.
    pub fn library_selected(&self) -> usize {
        self.library_selected
    }

    /// Selected queue row index; always in range when there are entries.
    pub fn queue_selected(&self) -> usize {
        self.queue_selected
    }

    fn selected_row(&self) -> Option<LibraryRow<'_>> {
        let dir = self.library_dir();
        let i = self.library_selected;
        match dir.dirs().get(i) {
            Some(d) => Some(LibraryRow::Dir(d)),
            None => dir
                .tracks()
                .get(i - dir.dirs().len())
                .map(LibraryRow::Track),
        }
    }

    fn selected_key(&self) -> Option<RowKey> {
        self.selected_row().map(|row| match row {
            LibraryRow::Dir(d) => RowKey::Dir(d.name().to_owned()),
            LibraryRow::Track(t) => RowKey::Track(t.local_path().unwrap_or(Path::new("")).into()),
        })
    }

    fn row_index(&self, key: &RowKey) -> Option<usize> {
        let dir = self.library_dir();
        match key {
            RowKey::Dir(name) => dir.dirs().iter().position(|d| d.name() == name),
            RowKey::Track(path) => dir
                .tracks()
                .iter()
                .position(|t| t.local_path() == Some(path.as_path()))
                .map(|i| i + dir.dirs().len()),
        }
    }

    // ----- inputs ------------------------------------------------------

    /// Applies a user action and returns the commands to send.
    pub fn handle_action(&mut self, action: Action, now: Instant) -> Vec<PlaybackCommand> {
        if self.help_open {
            match action {
                Action::ToggleHelp | Action::CloseHelp => self.help_open = false,
                Action::Quit => self.quit = true,
                _ => {}
            }
            return Vec::new();
        }
        let commands = match action {
            Action::Up => {
                self.move_selection(-1);
                Vec::new()
            }
            Action::Down => {
                self.move_selection(1);
                Vec::new()
            }
            Action::Activate => match self.focus {
                Focus::Library => self.activate_library(now),
                Focus::Queue => self.activate_queue(),
            },
            Action::Parent => {
                if self.focus == Focus::Library {
                    self.go_to_parent();
                }
                Vec::new()
            }
            Action::Enqueue => {
                if self.focus == Focus::Library {
                    self.enqueue_selected(now);
                }
                Vec::new()
            }
            Action::Remove => match self.focus {
                Focus::Queue => self.remove_selected(),
                Focus::Library => Vec::new(),
            },
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
                Vec::new()
            }
            Action::CloseHelp => Vec::new(),
            Action::Quit => {
                self.quit = true;
                Vec::new()
            }
        };
        let commands = resolve::dispatch(&mut self.player, self.navidrome.as_ref(), commands);
        self.after_player_call(now);
        commands
    }

    pub fn handle_playback_event(
        &mut self,
        event: PlaybackEvent,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        let commands = self.player.handle_event(event);
        let commands = resolve::dispatch(&mut self.player, self.navidrome.as_ref(), commands);
        self.after_player_call(now);
        commands
    }

    /// Shows the result of the startup Navidrome ping.
    pub fn handle_navidrome_ping(&mut self, result: Result<(), navidrome::Error>, now: Instant) {
        match result {
            Ok(()) => {
                tracing::info!("navidrome ping ok");
                self.set_status("Navidrome connected", now);
            }
            Err(error) => {
                // Never `Debug`: network errors include the server address.
                tracing::warn!(%error, detail = %error.detail(), "navidrome ping failed");
                self.set_status(format!("Navidrome: {error}"), now);
            }
        }
    }

    pub fn handle_scan_event(&mut self, event: ScanEvent, now: Instant) {
        match event {
            ScanEvent::Batch(tracks) => {
                let key = self.selected_key();
                self.tree.extend(tracks);
                if let ScanState::Scanning { tracks } = &mut self.scan {
                    *tracks = self.tree.len();
                }
                self.library_selected = key.and_then(|k| self.row_index(&k)).unwrap_or(0);
                self.clamp_selections();
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
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.status.as_ref().is_some_and(|s| now >= s.expires_at) {
            self.status = None;
            return true;
        }
        false
    }

    pub fn set_status(&mut self, text: impl Into<String>, now: Instant) {
        self.status = Some(StatusMessage {
            text: text.into(),
            expires_at: now + STATUS_TTL,
        });
    }

    // ----- helpers -----------------------------------------------------

    fn after_player_call(&mut self, now: Instant) {
        if let Some(last) = self.player.take_messages().pop() {
            self.set_status(last, now);
        }
        self.clamp_selections();
    }

    fn clamp_selections(&mut self) {
        self.library_selected = self
            .library_selected
            .min(self.library_len().saturating_sub(1));
        self.queue_selected = self
            .queue_selected
            .min(self.player.queue().len().saturating_sub(1));
    }

    fn move_selection(&mut self, delta: isize) {
        let len = match self.focus {
            Focus::Library => self.library_len(),
            Focus::Queue => self.player.queue().len(),
        };
        let selected = match self.focus {
            Focus::Library => &mut self.library_selected,
            Focus::Queue => &mut self.queue_selected,
        };
        *selected = selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    fn activate_library(&mut self, now: Instant) -> Vec<PlayerCommand> {
        match self.selected_row() {
            Some(LibraryRow::Dir(dir)) => {
                let name = dir.name().to_owned();
                self.library_path.push(name);
                self.library_selected = 0;
                Vec::new()
            }
            Some(LibraryRow::Track(track)) => {
                let track = track.clone();
                self.set_status(format!("Playing {}", track.display_name()), now);
                self.player.play_track_now(track)
            }
            None => Vec::new(),
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

    fn go_to_parent(&mut self) {
        let Some(left) = self.library_path.pop() else {
            return;
        };
        self.library_selected = self.row_index(&RowKey::Dir(left)).unwrap_or(0);
    }

    fn enqueue_selected(&mut self, now: Instant) {
        let tracks: Vec<Track> = match self.selected_row() {
            Some(LibraryRow::Track(track)) => vec![track.clone()],
            Some(LibraryRow::Dir(dir)) => {
                let mut tracks = Vec::new();
                collect_tracks(dir, &mut tracks);
                tracks
            }
            None => return,
        };
        let text = match tracks.as_slice() {
            [one] => format!("Added {}", one.display_name()),
            many => format!("Added {}", count(many.len(), "track")),
        };
        for track in tracks {
            self.player.enqueue(track);
        }
        self.set_status(text, now);
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

/// All tracks below `dir` in browse order: sub-directories first (each
/// recursively), then the directory's own tracks.
fn collect_tracks(dir: &DirNode, out: &mut Vec<Track>) {
    for sub in dir.dirs() {
        collect_tracks(sub, out);
    }
    out.extend(dir.tracks().iter().cloned());
}

/// `1 track`, `1,234 tracks`.
pub fn count(n: usize, noun: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{} {noun}{plural}", thousands(n))
}

/// Formats with `,` thousands separators.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

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
pub async fn run(
    mut playback: PlaybackHandle,
    music_dir: Option<PathBuf>,
    navidrome: Option<navidrome::Client>,
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
    let ping = navidrome.clone().map(spawn_ping);
    let mut app = App::with_sources(music_dir, navidrome);
    let sources = Sources { scan, ping };
    let result = event_loop(&mut terminal, &mut app, &mut playback, sources, signals).await;
    if let Err(error) = &result {
        tracing::error!(%error, "terminal error; quitting");
    }
    shutdown_backend(playback).await;
    release_terminal(ManuallyDrop::into_inner(terminal));
    restore_terminal();
    result
}

async fn shutdown_backend(playback: PlaybackHandle) {
    let outcome = playback.shutdown(SHUTDOWN_TIMEOUT).await;
    tracing::info!(?outcome, "playback backend shut down");
}

/// Shows the cursor and drops the terminal. Ratatui's `Terminal::drop`
/// retries a failed show-cursor with `eprintln!`, which panics when the
/// terminal has hung up; in that case the terminal is leaked instead (the
/// process is exiting).
fn release_terminal(mut terminal: DefaultTerminal) {
    if let Err(error) = terminal.show_cursor() {
        tracing::warn!(%error, "could not show the cursor");
        std::mem::forget(terminal);
    }
}

/// Restores the terminal without ever writing to stderr: after a hangup
/// the terminal is gone and a failed write must not panic.
fn restore_terminal() {
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
            let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
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
    ping: Option<oneshot::Receiver<PingResult>>,
}

/// Pings the server on a Tokio task; the result arrives over a oneshot.
fn spawn_ping(client: navidrome::Client) -> oneshot::Receiver<PingResult> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(client.ping().await);
    });
    rx
}

/// Waits for the ping result; pends forever once it has been taken.
async fn next_ping(ping: &mut Option<oneshot::Receiver<PingResult>>) -> Option<PingResult> {
    match ping {
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
) -> io::Result<()> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut view = render::ViewState::default();
    let Sources { mut scan, mut ping } = sources;
    let mut playback_open = true;
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| render::draw(frame, app, &mut view, Instant::now()))?;
            dirty = false;
        }
        tokio::select! {
            input_event = input.next() => match input_event {
                Some(Ok(Event::Key(key))) => {
                    if key.kind != KeyEventKind::Release
                        && let Some(action) = keymap::map_key(key, app.focus(), app.help_open())
                    {
                        let commands = app.handle_action(action, Instant::now());
                        playback.send_all(commands);
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
            result = next_ping(&mut ping) => {
                ping = None;
                // A dropped sender means the task panicked; it is logged.
                if let Some(result) = result {
                    app.handle_navidrome_ping(result, Instant::now());
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
                app.tick(Instant::now());
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
        app.player.enqueue(remote("r2"));
        app.player.enqueue(track("b.flac"));
        app.focus = Focus::Queue;
        // Both remote entries fail in turn; the local one is loaded.
        let commands = press(&mut app, &[Action::Activate], now);
        assert_eq!(load_path(&commands), PathBuf::from("/music/b.flac"));
        let current = app.player.queue().current_entry().unwrap();
        assert_eq!(current.track.title, "b");
        assert_eq!(
            app.status(now),
            Some("Could not play Remote r2: Navidrome is not configured")
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

    fn app_with(rels: &[&str]) -> (App, Instant) {
        let now = Instant::now();
        let mut app = App::new(ROOT.into());
        app.handle_scan_event(
            ScanEvent::Batch(rels.iter().map(|r| track(r)).collect()),
            now,
        );
        (app, now)
    }

    fn row_names(app: &App) -> Vec<String> {
        app.library_rows()
            .iter()
            .map(|row| match row {
                LibraryRow::Dir(d) => format!("{}/", d.name()),
                LibraryRow::Track(t) => t.title.clone(),
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
        // Parent at the root is a no-op.
        press(&mut app, &[Action::Parent], now);
        assert_eq!(selected_name(&app), "b/");
        // Nested.
        press(
            &mut app,
            &[Action::Up, Action::Activate, Action::Activate],
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
        let mut app = App::new(ROOT.into());
        let now = Instant::now();
        assert!(app.library_rows().is_empty());
        let commands = press(
            &mut app,
            &[
                Action::Down,
                Action::Activate,
                Action::Enqueue,
                Action::Parent,
            ],
            now,
        );
        assert!(commands.is_empty());
        assert_eq!(app.library_selected(), 0);
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
}
