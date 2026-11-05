//! Pure mpv backend state: turns [`PlaybackCommand`]s and parsed mpv
//! messages into mpv requests and [`PlaybackEvent`]s. No I/O and no Tokio;
//! the async driver feeds it and performs the writes.
//!
//! Stale-event core (M1 brief section 3.5):
//! 1. Loads are serialized. While one is in flight, further loads collapse
//!    into a single queued load (the latest wins).
//! 2. A load sends `set_property pause false` and `loadfile <path> replace`.
//!    `replace` leaves exactly one entry, and the reply names it
//!    (`playlist_entry_id`); that id is mapped to the load's [`LoadId`].
//! 3. File events are tagged with their playlist entry id. `start-file` and
//!    `end-file` carry it; `file-loaded` does not, so it is attributed to
//!    the entry of the preceding `start-file` (the entry being opened).
//!    Events for mapped ids become `Started` (on `file-loaded`) and
//!    `TrackEnded` (on `end-file`). Events for unmapped ids newer than any
//!    mapped id are buffered (bounded) while a mapping is pending and
//!    replayed once it is known; all others are dropped.
//! 4. A failed `loadfile` reply becomes `TrackEnded { Error }`.
//! 5. mpv may expand a file into several entries (content-detected
//!    playlists end with `redirect`). A mapped `redirect` is an error, and
//!    an unmapped `start-file` with no load in flight is stopped.
//!
//! Gapless lookahead (M8, ADR 0005): besides the current entry, mpv holds
//! at most a copy of the next one, which mpv plays on by itself.
//! 1. `SetNext { after, next }` applies only while `after` is the latest
//!    load (or advance). Once that load's entry is mapped, the backend
//!    sends `playlist-clear` (keeping the playing entry) and, for a next
//!    track, `loadfile <next> append`, whose reply maps the new entry. At
//!    most one such change is in flight; later ones collapse to the latest.
//!    Entries are never addressed by index (`playlist-remove 1` removes
//!    the playing entry once mpv has moved on).
//! 2. mpv picks the successor when the current entry ends, from its
//!    playlist at that moment. Replies do not tell whether a change came
//!    before that: mpv runs IPC commands synchronously and writes the
//!    events that happened meanwhile after the reply. So at the current
//!    entry's `end-file eof`, if any appended entry could follow, the
//!    backend asks `idle-active` and waits. The next `start-file` names
//!    the successor: an appended entry gives `Advanced { from, to }`
//!    (possibly a superseded one; the player corrects that). If instead
//!    mpv reports that it is idle, the current entry ended with
//!    `TrackEnded { Eof }` as before. mpv chooses the successor without
//!    running commands in between, so `idle-active` is never answered
//!    between `end-file` and the successor's `start-file`. That holds
//!    while no hook runs there (`on_after_end_file`; Coda starts mpv with
//!    `--no-config` and registers none). Should one ever run, mpv's `idle`
//!    event also ends the wait, so it cannot hang.
//! 3. An advanced-to entry is heard only after the previous one's buffered
//!    audio has played out (about 0.25 s, seconds on slow outputs). Its
//!    `Started` waits for the first `time-pos` above zero after its
//!    `file-loaded` (mpv 0.33–0.35 report negative values until then,
//!    0.41 reports 0 at `file-loaded` and nothing until it is heard), or
//!    comes just before its `end-file` if that is first (a very short
//!    track). Loads made with `replace` keep `Started` at `file-loaded`.
//! 4. If mpv answers a `loadfile` without an entry id, the lookahead is
//!    off for the session and loads fall back to querying
//!    `playlist/0/id`, as before M8.
//!
//! Reply watchdog: the driver stamps each request when it hands it to the
//! writer ([`MpvState::requests_sent`]) and asks
//! [`MpvState::overdue_reply`] at least every [`POSITION_INTERVAL`]. mpv
//! usually answers at once, even `loadfile` of a stream that stalls
//! (probed on mpv 0.41). It does block its core while draining the audio
//! output's buffer after the last entry ends: replies then wait as long as
//! the buffered audio lasts (seen: about 2 s on real devices, 3 s with
//! `--ao-null-buffer=3`). [`REPLY_TIMEOUT`] leaves room for that; a reply
//! missing for that long means mpv is hung. Time the driver itself was not
//! reading from mpv does not count.
//!
//! Visualizer: [`super::visualizer`] decides when to add or remove its
//! filter graph and when to poll; this state sends those requests, feeds it
//! mpv's pause, position, sample rate and file events, and passes its
//! readings on as [`PlaybackEvent::Visualizer`]. The driver calls
//! [`MpvState::visualizer_tick`] at [`MpvState::visualizer_deadline`]. A
//! gapless change hands the visualizer over without removing its graph.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::protocol::{
    self, EndFileReason, MpvCommand, MpvEvent, MpvMessage, ProtocolError, SeekMode,
};
use super::visualizer::{self, Visualizer};
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackTarget};

/// Minimum spacing between emitted `Position` events (at most 4/s).
pub const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// Bound on file events buffered while an entry-id mapping is pending.
pub const MAX_BUFFERED_EVENTS: usize = 16;

/// Bound on remembered entry-id mappings. Mappings are removed on
/// `end-file`, and appended entries that can no longer play once a later
/// entry starts; this only guards against unbounded growth.
const MAX_ENTRY_MAPPINGS: usize = 64;

/// Queried for the new entry's id only when mpv's `loadfile` reply does not
/// carry it (the fallback without lookahead).
pub const ENTRY_ID_PROPERTY: &str = "playlist/0/id";
/// Asked at the end of an entry that mpv may have continued from: `true`
/// means mpv stopped (no successor).
pub const IDLE_PROPERTY: &str = "idle-active";
pub const OBSERVED_PROPERTIES: [&str; 5] = [
    "time-pos",
    "duration",
    "pause",
    "volume",
    SAMPLE_RATE_PROPERTY,
];
/// The current track's decoded sample rate; the visualizer hides bands it
/// cannot contain.
pub const SAMPLE_RATE_PROPERTY: &str = "audio-params/samplerate";
pub const FILE_NOT_FOUND: &str = "file not found";
pub const NOT_A_REGULAR_FILE: &str = "not a regular file";
pub const NOT_AUDIO: &str = "not an audio file (playlist or directory)";

/// How long a request may go unanswered before mpv is considered hung.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// The `BackendExited` reason when mpv stops answering requests.
pub const STOPPED_RESPONDING: &str = "mpv stopped responding";

/// The driver checks the watchdog at least every [`POSITION_INTERVAL`]. A
/// longer gap between two checks means it was not reading from mpv (blocked
/// delivering events to a stalled consumer, prechecking a file on a slow
/// mount, or the whole process was suspended); that time is not held
/// against mpv.
pub const MAX_CHECK_GAP: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingRequest {
    pub request_id: u64,
    pub command: MpvCommand,
}

impl OutgoingRequest {
    pub fn encode(&self) -> String {
        protocol::encode_request(self.request_id, &self.command)
    }
}

/// What the driver must do after a state transition: write `requests` in
/// order, then deliver `events` in order.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub requests: Vec<OutgoingRequest>,
    pub events: Vec<PlaybackEvent>,
}

/// What a pending request's reply means.
#[derive(Debug, Clone, Copy)]
enum Pending {
    /// Failure is logged only.
    Ignore,
    /// Failure is reported to the user as `PlaybackEvent::Error`.
    Report(&'static str),
    /// `set_property volume` to this value; failure is reported as
    /// `PlaybackEvent::VolumeRejected`.
    Volume(u8),
    /// `loadfile … replace` of this load; the reply names its entry.
    LoadFile(LoadId),
    /// `get_property playlist/0/id` for this load (fallback only).
    EntryId(LoadId),
    /// `playlist-clear` of a set-next.
    NextClear,
    /// `loadfile … append` of this next load; the reply names its entry.
    NextAppend(LoadId),
    /// `get_property idle-active` at the end of this entry.
    SwitchCheck(i64),
    VisualizerAttach,
    /// A visualizer poll of this generation.
    VisualizerPoll(u64),
}

#[derive(Debug)]
struct InFlight {
    load: LoadId,
}

/// A file event attributed to a playlist entry.
#[derive(Debug, Clone, PartialEq)]
enum FileEventKind {
    Start,
    Loaded,
    End {
        reason: EndFileReason,
        file_error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct FileEvent {
    entry_id: i64,
    kind: FileEventKind,
}

/// A playlist entry Coda requested.
#[derive(Debug, Clone, Copy)]
struct Mapped {
    load: LoadId,
    /// mpv has sent its `start-file`.
    started: bool,
}

/// The current entry ended at EOF while appended entries could follow it;
/// waiting for mpv's next `start-file` or its `idle-active` answer.
#[derive(Debug, Clone, Copy)]
struct Switching {
    entry: i64,
    load: LoadId,
}

/// An entry mpv advanced to by itself, not heard yet.
#[derive(Debug, Clone, Copy)]
struct Unheard {
    entry: i64,
    load: LoadId,
    /// Its `file-loaded` arrived.
    loaded: bool,
}

#[derive(Debug, Default)]
struct PositionCoalescer {
    last_emit: Option<Instant>,
    pending: Option<Duration>,
}

impl PositionCoalescer {
    fn due(&self, now: Instant) -> bool {
        self.last_emit
            .is_none_or(|t| now.saturating_duration_since(t) >= POSITION_INTERVAL)
    }

    fn offer(&mut self, position: Duration, now: Instant) -> Option<Duration> {
        if self.due(now) {
            self.last_emit = Some(now);
            self.pending = None;
            Some(position)
        } else {
            self.pending = Some(position);
            None
        }
    }

    fn flush(&mut self, now: Instant) -> Option<Duration> {
        if self.pending.is_some() && self.due(now) {
            self.last_emit = Some(now);
            self.pending.take()
        } else {
            None
        }
    }

    /// Drops a not-yet-emitted position so an old track's progress is never
    /// delivered after a track boundary.
    fn clear(&mut self) {
        self.pending = None;
    }
}

/// Send times of requests still waiting for their reply. Pure: the caller
/// supplies every `now`. Request ids and send times both only increase, so
/// the lowest id is the oldest request.
#[derive(Debug)]
struct ReplyWatchdog {
    timeout: Duration,
    sent: BTreeMap<u64, Instant>,
    last_check: Option<Instant>,
}

impl Default for ReplyWatchdog {
    fn default() -> Self {
        Self::new(REPLY_TIMEOUT)
    }
}

impl ReplyWatchdog {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            sent: BTreeMap::new(),
            last_check: None,
        }
    }

    fn sent(&mut self, request_id: u64, now: Instant) {
        self.sent.insert(request_id, now);
    }

    fn answered(&mut self, request_id: u64) {
        self.sent.remove(&request_id);
    }

    /// The oldest unanswered request and how long it has waited, if that
    /// is at least the timeout. A gap since the previous check longer than
    /// [`MAX_CHECK_GAP`] is excused: every send time moves forward by it.
    /// Requests are stamped with the time of a check, so none is newer than
    /// the previous check, none moves past `now`, and their order holds.
    fn check(&mut self, now: Instant) -> Option<(u64, Duration)> {
        let gap = self
            .last_check
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.last_check = Some(now);
        if gap > MAX_CHECK_GAP {
            tracing::debug!(?gap, "not reading from mpv for a while; excused");
            for sent in self.sent.values_mut() {
                *sent = sent.checked_add(gap).unwrap_or(now).min(now);
            }
        }
        self.sent
            .first_key_value()
            .map(|(&id, &sent)| (id, now.saturating_duration_since(sent)))
            .filter(|&(_, waited)| waited >= self.timeout)
    }
}

#[derive(Debug, Default)]
pub struct MpvState {
    next_request_id: u64,
    pending: HashMap<u64, Pending>,
    watchdog: ReplyWatchdog,
    in_flight: Option<InFlight>,
    queued: Option<(LoadId, PlaybackTarget)>,
    entries: HashMap<i64, Mapped>,
    /// Highest entry id ever mapped. mpv entry ids only increase, so an
    /// unmapped id at or below this can never become ours.
    max_mapped: Option<i64>,
    /// Entry of the latest `start-file` that has not ended yet; receives
    /// the next `file-loaded`.
    opening: Option<i64>,
    buffered: VecDeque<FileEvent>,
    position: PositionCoalescer,
    visualizer: Visualizer,
    /// mpv answered a `loadfile` without an entry id: no lookahead this
    /// session.
    lookahead_off: bool,
    /// The load the player last made current: by `Load`, or by a gapless
    /// advance this backend reported. `None` after `Stop`, after a load
    /// that failed or never reached mpv (rejected by the driver), and after
    /// the latest load's end. `SetNext` for any other load is stale.
    latest: Option<LoadId>,
    /// A set-next waiting to be sent (`Some(None)`: clear the next entry).
    wanted_next: Option<Option<(LoadId, PlaybackTarget)>>,
    /// The next load mpv was last asked to hold after `latest`.
    sent_next: Option<LoadId>,
    /// Replies still due for set-next requests; none is sent meanwhile.
    next_replies_due: usize,
    switching: Option<Switching>,
    unheard: Option<Unheard>,
    /// After a gapless advance, mpv may still hold an entry after
    /// `latest` that nobody asked for: "nothing next" must then send a
    /// clear even though `sent_next` is `None`.
    unwanted_next: bool,
}

impl MpvState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A state whose watchdog allows `timeout` per reply instead of
    /// [`REPLY_TIMEOUT`] (tests use short timeouts).
    pub fn with_reply_timeout(timeout: Duration) -> Self {
        Self {
            watchdog: ReplyWatchdog::new(timeout),
            ..Self::default()
        }
    }

    /// A state whose visualizer adds `graph` instead of
    /// [`visualizer::GRAPH`] (tests use a broken one).
    pub fn with_visualizer_graph(mut self, graph: String) -> Self {
        self.visualizer = Visualizer::new(graph);
        self
    }

    /// Records that `requests` were handed to the writer at `now`. Call it
    /// with the `now` of the latest [`Self::overdue_reply`] check.
    pub fn requests_sent(&mut self, requests: &[OutgoingRequest], now: Instant) {
        for request in requests {
            self.watchdog.sent(request.request_id, now);
        }
    }

    /// Checks the reply watchdog; call it at least every
    /// [`POSITION_INTERVAL`] while reading from mpv. Returns the oldest
    /// request that has gone unanswered for the timeout, with its wait.
    pub fn overdue_reply(&mut self, now: Instant) -> Option<(u64, Duration)> {
        self.watchdog.check(now)
    }

    /// Requests to send once connected: observe the reported properties.
    pub fn startup(&mut self) -> Output {
        let mut out = Output::default();
        for (i, name) in OBSERVED_PROPERTIES.into_iter().enumerate() {
            let id = i as u64 + 1;
            self.request(
                &mut out,
                MpvCommand::ObserveProperty { id, name },
                Pending::Ignore,
            );
        }
        out
    }

    /// `true` while a load is waiting for its entry id.
    pub fn load_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// `false` once mpv showed it cannot support the gapless lookahead.
    pub fn lookahead_enabled(&self) -> bool {
        !self.lookahead_off
    }

    pub fn handle_command(&mut self, command: PlaybackCommand) -> Output {
        let mut out = Output::default();
        match command {
            PlaybackCommand::Load { load, target } => {
                self.set_latest(Some(load));
                if self.in_flight.is_some() {
                    // Collapse: only the latest queued load survives.
                    if let Some((dropped, _)) = self.queued.replace((load, target)) {
                        tracing::debug!(%load, %dropped, "load superseded before reaching mpv");
                    }
                } else {
                    self.start_load(&mut out, load, target);
                }
            }
            PlaybackCommand::SetNext { after, next } => self.set_next(&mut out, after, next),
            PlaybackCommand::SetPause(paused) => self.request(
                &mut out,
                MpvCommand::SetProperty {
                    name: "pause",
                    value: paused.into(),
                },
                Pending::Report("pause"),
            ),
            PlaybackCommand::Stop => {
                if let Some((dropped, _)) = self.queued.take() {
                    tracing::debug!(%dropped, "queued load cancelled by stop");
                }
                self.set_latest(None);
                self.visualizer_track_change(&mut out, None);
                // `stop` also clears mpv's playlist.
                self.request(&mut out, MpvCommand::Stop, Pending::Ignore);
            }
            PlaybackCommand::SetVisualizer(on) => {
                self.visualizer.set_wanted(on);
                self.visualizer_attach(&mut out);
            }
            PlaybackCommand::SeekRelative(seconds) => {
                self.seek(&mut out, seconds, SeekMode::Relative);
            }
            PlaybackCommand::SeekAbsolute(seconds) => {
                self.seek(&mut out, seconds, SeekMode::Absolute);
            }
            PlaybackCommand::SetVolume(volume) => {
                let volume = volume.min(100);
                self.request(
                    &mut out,
                    MpvCommand::SetProperty {
                        name: "volume",
                        value: volume.into(),
                    },
                    Pending::Volume(volume),
                );
            }
            PlaybackCommand::Shutdown => self.request(&mut out, MpvCommand::Quit, Pending::Ignore),
        }
        out
    }

    /// The driver rejected `load` before it reached mpv (for example the
    /// path does not exist or is not a regular file). The load supersedes
    /// anything queued but never reaches mpv.
    pub fn load_rejected(&mut self, load: LoadId, reason: &str) -> Output {
        let mut out = Output::default();
        self.reject_load(&mut out, load, reason.to_owned());
        out
    }

    /// Shorthand for [`Self::load_rejected`] with [`FILE_NOT_FOUND`].
    pub fn load_missing(&mut self, load: LoadId) -> Output {
        self.load_rejected(load, FILE_NOT_FOUND)
    }

    /// Parses and handles one line from mpv.
    pub fn handle_line(&mut self, line: &str, now: Instant) -> Result<Output, ProtocolError> {
        let message = protocol::parse_line(line)?;
        Ok(self.handle_message(message, now))
    }

    pub fn handle_message(&mut self, message: MpvMessage, now: Instant) -> Output {
        let mut out = Output::default();
        match message {
            MpvMessage::Reply { request_id, result } => {
                self.on_reply(&mut out, request_id, result, now);
            }
            MpvMessage::Event(event) => self.on_event(&mut out, event, now),
        }
        out
    }

    /// When [`Self::visualizer_tick`] is due; `None` while the visualizer
    /// has nothing to do (hidden, paused, stopped or unavailable).
    pub fn visualizer_deadline(&self, now: Instant) -> Option<Instant> {
        self.visualizer.deadline(now)
    }

    /// Shows the newest due visualizer reading and polls for the next.
    pub fn visualizer_tick(&mut self, now: Instant) -> Output {
        let mut out = Output::default();
        let tick = self.visualizer.tick(now);
        out.events.extend(tick.event);
        if let Some(generation) = tick.poll {
            self.request(
                &mut out,
                MpvCommand::GetProperty {
                    name: visualizer::METADATA_PROPERTY,
                },
                Pending::VisualizerPoll(generation),
            );
        }
        out
    }

    /// Periodic flush of a coalesced position update.
    pub fn tick(&mut self, now: Instant) -> Output {
        let mut out = Output::default();
        if let Some(position) = self.position.flush(now) {
            out.events.push(PlaybackEvent::Position(position));
        }
        out
    }

    fn request(&mut self, out: &mut Output, command: MpvCommand, pending: Pending) {
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        self.pending.insert(request_id, pending);
        out.requests.push(OutgoingRequest {
            request_id,
            command,
        });
    }

    fn seek(&mut self, out: &mut Output, seconds: f64, mode: SeekMode) {
        if seconds.is_finite() {
            self.visualizer.seek_sent();
            self.request(out, MpvCommand::Seek { seconds, mode }, Pending::Ignore);
        }
    }

    /// Adds the visualizer graph if it is wanted and a track is playing.
    fn visualizer_attach(&mut self, out: &mut Output) {
        if self.in_flight.is_none()
            && self.visualizer.is_playing()
            && let Some(graph) = self.visualizer.take_attach()
        {
            self.request(out, MpvCommand::AfAdd { graph }, Pending::VisualizerAttach);
        }
    }

    /// A load (`Some`) or stop (`None`) is about to be sent; removes a
    /// hidden graph first.
    fn visualizer_track_change(&mut self, out: &mut Output, next: Option<LoadId>) {
        if self.visualizer.track_change(next) {
            self.visualizer_detach(out);
        }
    }

    fn visualizer_detach(&mut self, out: &mut Output) {
        self.request(
            out,
            MpvCommand::AfRemove {
                labels: visualizer::GRAPH_LABELS,
            },
            Pending::Ignore,
        );
    }

    /// The player's current load changed (`None`: stopped, or nothing
    /// reached mpv). A wanted next entry belongs to the previous one, and
    /// mpv holds none after the new one yet.
    fn set_latest(&mut self, load: Option<LoadId>) {
        self.latest = load;
        self.wanted_next = None;
        self.sent_next = None;
        self.unwanted_next = false;
        self.unheard = None;
    }

    fn start_load(&mut self, out: &mut Output, load: LoadId, target: PlaybackTarget) {
        let command = match target {
            PlaybackTarget::File(path) => match path.to_str() {
                Some(path) => MpvCommand::LoadFile {
                    path: path.to_owned(),
                },
                None => {
                    self.reject_load(out, load, "path is not valid UTF-8".to_owned());
                    return;
                }
            },
            PlaybackTarget::Stream(url) => MpvCommand::LoadStream { url },
        };
        tracing::debug!(%load, "sending load to mpv");
        self.position.clear();
        self.visualizer_track_change(out, Some(load));
        self.in_flight = Some(InFlight { load });
        self.request(
            out,
            MpvCommand::SetProperty {
                name: "pause",
                value: false.into(),
            },
            Pending::Ignore,
        );
        self.request(out, command, Pending::LoadFile(load));
    }

    fn reject_load(&mut self, out: &mut Output, load: LoadId, message: String) {
        if let Some((dropped, _)) = self.queued.take() {
            tracing::debug!(%dropped, %load, "queued load superseded by rejected load");
        }
        tracing::debug!(%load, reason = %message, "load rejected before reaching mpv");
        // Nothing of it is in mpv, so nothing can follow it there; and the
        // player has moved on from whatever was current before it.
        self.set_latest(None);
        self.position.clear();
        out.events.push(PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Error(message),
        });
    }

    // ----- gapless lookahead -------------------------------------------

    fn set_next(
        &mut self,
        out: &mut Output,
        after: LoadId,
        next: Option<(LoadId, PlaybackTarget)>,
    ) {
        if self.lookahead_off {
            return;
        }
        if self.latest != Some(after) {
            tracing::debug!(%after, "next entry for a load that is no longer current ignored");
            return;
        }
        let load = next.as_ref().map(|(load, _)| *load);
        if load == self.sent_next && !(load.is_none() && self.unwanted_next) {
            // What mpv holds already (a change still waiting is dropped).
            self.wanted_next = None;
            return;
        }
        self.wanted_next = Some(next);
        self.send_next(out);
    }

    /// Sends the wanted next entry once the current entry is in mpv and no
    /// other set-next is in flight.
    fn send_next(&mut self, out: &mut Output) {
        if self.lookahead_off
            || self.next_replies_due > 0
            || self.in_flight.is_some()
            || self.queued.is_some()
            || self.switching.is_some()
            || self.latest.is_none()
        {
            return;
        }
        let Some(next) = self.wanted_next.take() else {
            return;
        };
        let append = next.and_then(|(load, target)| {
            let command = match target {
                PlaybackTarget::File(path) => MpvCommand::AppendFile {
                    path: path.to_str()?.to_owned(),
                },
                PlaybackTarget::Stream(url) => MpvCommand::AppendStream { url },
            };
            Some((load, command))
        });
        let load = append.as_ref().map(|(load, _)| *load);
        if load == self.sent_next && !(load.is_none() && self.unwanted_next) {
            return;
        }
        self.unwanted_next = false;
        match load {
            Some(load) => tracing::debug!(%load, "sending the next entry to mpv"),
            None => tracing::debug!("clearing mpv's next entry"),
        }
        self.request(out, MpvCommand::PlaylistClear, Pending::NextClear);
        self.next_replies_due += 1;
        if let Some((load, command)) = append {
            self.request(out, command, Pending::NextAppend(load));
            self.next_replies_due += 1;
        }
        self.sent_next = load;
    }

    /// mpv answered a `loadfile` without the new entry's id (an mpv older
    /// than any tested): the lookahead cannot map entries, so it is off.
    fn lookahead_unsupported(&mut self) {
        if !self.lookahead_off {
            tracing::warn!(
                "mpv did not report playlist entry ids; gapless playback is off for this session"
            );
        }
        self.lookahead_off = true;
        self.wanted_next = None;
        self.sent_next = None;
    }

    /// Appended entries after `entry` that mpv could play next.
    fn could_follow(&self, entry: i64) -> bool {
        self.entries
            .iter()
            .any(|(&id, mapped)| id > entry && !mapped.started)
    }

    fn on_reply(
        &mut self,
        out: &mut Output,
        request_id: u64,
        result: Result<Option<Value>, String>,
        now: Instant,
    ) {
        self.watchdog.answered(request_id);
        let Some(pending) = self.pending.remove(&request_id) else {
            tracing::debug!(request_id, "reply for unknown request ignored");
            return;
        };
        match pending {
            Pending::Ignore => {
                if let Err(error) = result {
                    tracing::debug!(request_id, %error, "mpv request failed");
                }
            }
            Pending::Report(what) => {
                if let Err(error) = result {
                    tracing::warn!(request_id, %error, what, "mpv request failed");
                    out.events.push(PlaybackEvent::Error(format!(
                        "Could not set {what}: {error}"
                    )));
                }
            }
            Pending::Volume(volume) => {
                if let Err(reason) = result {
                    tracing::warn!(request_id, %reason, volume, "mpv rejected volume");
                    out.events
                        .push(PlaybackEvent::VolumeRejected { volume, reason });
                }
            }
            Pending::LoadFile(load) => self.on_loadfile(out, load, result),
            Pending::EntryId(load) => {
                let entry = result.ok().flatten().and_then(|v| v.as_i64());
                if entry.is_none() {
                    tracing::warn!(%load, "mpv did not report the new playlist entry");
                    self.load_failed(out, load, "mpv did not report the playlist entry".into());
                }
                self.finish_load(out, load, entry);
            }
            Pending::NextClear => {
                if let Err(error) = result {
                    tracing::debug!(%error, "mpv refused playlist-clear");
                }
                self.next_reply(out);
            }
            Pending::NextAppend(load) => {
                match result {
                    Ok(data) => match entry_id(data.as_ref()) {
                        Some(entry) => {
                            tracing::debug!(%load, entry, "mapped appended playlist entry");
                            self.forget_unreachable_appends();
                            self.map_entry(entry, load);
                        }
                        None => {
                            self.lookahead_unsupported();
                            // Remove what was appended: mpv must not play
                            // an entry Coda cannot recognise.
                            self.request(out, MpvCommand::PlaylistClear, Pending::Ignore);
                        }
                    },
                    Err(error) => {
                        // Not held; its own load reports the problem later.
                        tracing::info!(%load, %error, "mpv refused the next entry");
                        if self.sent_next == Some(load) {
                            self.sent_next = None;
                        }
                    }
                }
                self.next_reply(out);
            }
            Pending::SwitchCheck(entry) => self.on_switch_check(out, entry, result),
            Pending::VisualizerAttach => {
                if self.visualizer.attach_answered(result.map(|_| ())) {
                    out.events.push(PlaybackEvent::VisualizerUnavailable);
                }
            }
            Pending::VisualizerPoll(generation) => {
                self.visualizer.poll_answered(generation, result, now);
            }
        }
    }

    fn next_reply(&mut self, out: &mut Output) {
        self.next_replies_due = self.next_replies_due.saturating_sub(1);
        self.send_next(out);
    }

    fn on_loadfile(
        &mut self,
        out: &mut Output,
        load: LoadId,
        result: Result<Option<Value>, String>,
    ) {
        if !self.in_flight.as_ref().is_some_and(|f| f.load == load) {
            // Unreachable by construction (one load in flight); be safe.
            tracing::warn!(%load, "loadfile reply for a load that is not in flight");
            return;
        }
        match result {
            Ok(data) => match entry_id(data.as_ref()) {
                Some(entry) => self.finish_load(out, load, Some(entry)),
                None => {
                    // Before M8 the entry id was always queried; do that.
                    self.lookahead_unsupported();
                    self.request(
                        out,
                        MpvCommand::GetProperty {
                            name: ENTRY_ID_PROPERTY,
                        },
                        Pending::EntryId(load),
                    );
                }
            },
            Err(error) => {
                tracing::warn!(%load, %error, "mpv rejected loadfile");
                self.load_failed(out, load, format!("mpv could not load the file: {error}"));
                self.finish_load(out, load, None);
            }
        }
    }

    /// Reports `load` as failed before any of its file events.
    fn load_failed(&mut self, out: &mut Output, load: LoadId, message: String) {
        if self.latest == Some(load) {
            self.set_latest(None);
        }
        self.position.clear();
        out.events.push(PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Error(message),
        });
    }

    /// The load in flight is mapped to `entry`, or failed (`None`): replays
    /// its buffered events and sends what waited for it.
    fn finish_load(&mut self, out: &mut Output, load: LoadId, entry: Option<i64>) {
        self.in_flight = None;
        let buffered = std::mem::take(&mut self.buffered);
        if let Some(entry) = entry {
            tracing::debug!(%load, entry, "mapped mpv playlist entry");
            self.map_entry(entry, load);
            for event in buffered {
                if event.entry_id == entry {
                    self.deliver(out, event, load);
                } else {
                    tracing::debug!(?event, "dropping buffered event for unmapped entry");
                }
            }
        }
        if let Some((next, target)) = self.queued.take() {
            self.start_load(out, next, target);
        }
        self.send_next(out);
        // Once, after the replayed events (and only if what they started
        // is still playing: a queued load just superseded it).
        self.visualizer_attach(out);
    }

    fn map_entry(&mut self, entry_id: i64, load: LoadId) {
        // Only an entry that never started and is not the current one can
        // be dropped: their events must still be recognised.
        if self.entries.len() >= MAX_ENTRY_MAPPINGS
            && let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, m)| !m.started && Some(m.load) != self.latest)
                .map(|(&id, _)| id)
                .min()
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            entry_id,
            Mapped {
                load,
                started: false,
            },
        );
        self.max_mapped = Some(self.max_mapped.map_or(entry_id, |m| m.max(entry_id)));
    }

    /// A new append was answered, so the `playlist-clear` sent just before
    /// it has run: of the earlier appends that never started, only the
    /// newest can still play (mpv may have moved to it before the clear
    /// ran, its `start-file` still on the way). Older ones are gone from
    /// mpv's playlist. With the new entry, at most two unstarted appends
    /// stay mapped, however often the next entry changes within a track.
    fn forget_unreachable_appends(&mut self) {
        let latest = self.latest;
        let newest = self
            .entries
            .iter()
            .filter(|(_, m)| !m.started && Some(m.load) != latest)
            .map(|(&id, _)| id)
            .max();
        // The current entry may not have sent its `start-file` yet.
        self.entries.retain(|&id, mapped| {
            mapped.started || Some(mapped.load) == latest || Some(id) == newest
        });
    }

    /// mpv answered `idle-active` at the end of `entry`.
    fn on_switch_check(
        &mut self,
        out: &mut Output,
        entry: i64,
        result: Result<Option<Value>, String>,
    ) {
        let Some(switching) = self.switching.filter(|s| s.entry == entry) else {
            return;
        };
        if result.as_ref().ok().and_then(|d| d.as_ref()?.as_bool()) == Some(false) {
            // mpv went on to an entry; its `start-file` was sent before
            // this reply ran, and says which.
            return;
        }
        tracing::debug!(load = %switching.load, "mpv did not continue to an appended entry");
        self.switching = None;
        self.end_without_successor(out, switching);
        // A next entry for a newer load may have waited for the switch.
        self.send_next(out);
    }

    /// The entry of `switching` ended and mpv did not go on to an entry
    /// that follows it: an ordinary end.
    fn end_without_successor(&mut self, out: &mut Output, switching: Switching) {
        let end = FileEventKind::End {
            reason: EndFileReason::Eof,
            file_error: None,
        };
        self.deliver_end(out, switching.load, end);
    }

    fn on_event(&mut self, out: &mut Output, event: MpvEvent, now: Instant) {
        let file_event = match event {
            MpvEvent::StartFile {
                entry_id: Some(entry_id),
            } => {
                self.opening = Some(entry_id);
                if self.on_start(out, entry_id) {
                    return;
                }
                FileEvent {
                    entry_id,
                    kind: FileEventKind::Start,
                }
            }
            MpvEvent::FileLoaded => match self.opening {
                Some(entry_id) => {
                    if let Some(unheard) = self.unheard.as_mut().filter(|u| u.entry == entry_id) {
                        // `Started` waits until it is heard.
                        unheard.loaded = true;
                        return;
                    }
                    FileEvent {
                        entry_id,
                        kind: FileEventKind::Loaded,
                    }
                }
                None => {
                    tracing::debug!("file-loaded without a preceding start-file ignored");
                    return;
                }
            },
            MpvEvent::EndFile {
                entry_id: Some(entry_id),
                reason,
                file_error,
            } => {
                if self.opening == Some(entry_id) {
                    self.opening = None;
                }
                self.heard_at_end(out, entry_id);
                if self.wait_for_successor(out, entry_id, &reason) {
                    return;
                }
                FileEvent {
                    entry_id,
                    kind: FileEventKind::End { reason, file_error },
                }
            }
            MpvEvent::StartFile { entry_id: None } | MpvEvent::EndFile { entry_id: None, .. } => {
                tracing::debug!("file event without entry id ignored");
                return;
            }
            MpvEvent::PropertyChange { name, data } => {
                self.on_property(out, &name, data, now);
                return;
            }
            MpvEvent::Seek => {
                self.visualizer.seeking();
                return;
            }
            MpvEvent::PlaybackRestart => {
                self.visualizer.restarted();
                return;
            }
            MpvEvent::Other { name } => {
                // mpv went idle: nothing followed the entry that ended. With
                // no hooks, `idle-active` already says so; this covers a
                // hook running between `end-file` and the next `start-file`.
                if name == "idle"
                    && let Some(switching) = self.switching.take()
                {
                    tracing::debug!(load = %switching.load, "mpv went idle after the end");
                    self.end_without_successor(out, switching);
                    self.send_next(out);
                }
                return;
            }
        };
        self.route(out, file_event);
    }

    /// A `start-file` for `entry`. Settles a pending switch; returns `true`
    /// if the event was fully handled (a gapless advance).
    fn on_start(&mut self, out: &mut Output, entry: i64) -> bool {
        // Appended entries before this one were removed (by a clear or a
        // replace) and can never start.
        self.entries
            .retain(|&id, mapped| id >= entry || mapped.started);
        let Some(switching) = self.switching.take() else {
            return false;
        };
        let successor = self
            .entries
            .get(&entry)
            .filter(|m| !m.started && entry > switching.entry)
            .map(|m| m.load);
        match successor {
            Some(to) if self.latest == Some(switching.load) => {
                tracing::debug!(from = %switching.load, %to, "mpv continued without a gap");
                if let Some(mapped) = self.entries.get_mut(&entry) {
                    mapped.started = true;
                }
                self.position.clear();
                self.set_latest(Some(to));
                // An append for the previous track still unanswered, or
                // answered but behind `to` (mpv moved on before the clear
                // ran), would follow `to` in mpv: clear it unless the
                // player announces something else first.
                let behind = self.next_replies_due > 0
                    || self.entries.iter().any(|(&id, m)| id > entry && !m.started);
                if behind {
                    self.unwanted_next = true;
                    self.wanted_next = Some(None);
                }
                self.unheard = Some(Unheard {
                    entry,
                    load: to,
                    loaded: false,
                });
                // The graph stays; the readings are handed over when `to`
                // is heard.
                self.visualizer.gapless_change(to);
                out.events.push(PlaybackEvent::Advanced {
                    from: switching.load,
                    to,
                });
                self.send_next(out);
                true
            }
            _ => {
                // Not a successor the player asked for (or the player has
                // moved on): `switching.load` simply ended.
                self.end_without_successor(out, switching);
                self.send_next(out);
                false
            }
        }
    }

    /// The end of `entry`: an entry mpv advanced to that was never heard
    /// is reported as started first (a very short track).
    fn heard_at_end(&mut self, out: &mut Output, entry: i64) {
        if let Some(unheard) = self.unheard.filter(|u| u.entry == entry) {
            self.unheard = None;
            if unheard.loaded {
                self.heard(out, unheard.load);
            }
        }
    }

    fn heard(&mut self, out: &mut Output, load: LoadId) {
        tracing::debug!(%load, "the next track is heard");
        out.events.push(PlaybackEvent::Started { load });
        self.visualizer.started(load);
    }

    /// At the EOF of the current entry, if mpv may go on to an appended
    /// entry, starts waiting for its decision and returns `true`.
    fn wait_for_successor(&mut self, out: &mut Output, entry: i64, reason: &EndFileReason) -> bool {
        let Some(mapped) = self.entries.get(&entry).copied() else {
            return false;
        };
        let current = self.latest == Some(mapped.load);
        if *reason != EndFileReason::Eof || !current || !self.could_follow(entry) {
            return false;
        }
        self.entries.remove(&entry);
        self.position.clear();
        self.switching = Some(Switching {
            entry,
            load: mapped.load,
        });
        self.request(
            out,
            MpvCommand::GetProperty {
                name: IDLE_PROPERTY,
            },
            Pending::SwitchCheck(entry),
        );
        true
    }

    fn route(&mut self, out: &mut Output, event: FileEvent) {
        if let Some(mapped) = self.entries.get(&event.entry_id).copied() {
            self.deliver(out, event, mapped.load);
            self.visualizer_attach(out);
            return;
        }
        let mapping_pending = self.in_flight.is_some();
        let could_be_ours = self.max_mapped.is_none_or(|m| event.entry_id > m);
        if mapping_pending && could_be_ours {
            if self.buffered.len() >= MAX_BUFFERED_EVENTS {
                tracing::warn!(
                    entry_id = event.entry_id,
                    "event buffer full; dropping newest event"
                );
            } else {
                self.buffered.push_back(event);
            }
            return;
        }
        tracing::debug!(
            entry_id = event.entry_id,
            "event for unmapped entry dropped"
        );
        if event.kind == FileEventKind::Start && self.in_flight.is_none() {
            // mpv started an entry Coda never requested (for example one
            // inserted by a playlist file). Coda's queue is authoritative.
            tracing::warn!(
                entry_id = event.entry_id,
                "mpv started an unrequested entry; stopping it"
            );
            self.request(out, MpvCommand::Stop, Pending::Ignore);
        }
    }

    fn deliver(&mut self, out: &mut Output, event: FileEvent, load: LoadId) {
        self.position.clear();
        match event.kind {
            FileEventKind::Start => {
                tracing::debug!(%load, "mpv opening file");
                if let Some(mapped) = self.entries.get_mut(&event.entry_id) {
                    mapped.started = true;
                }
            }
            FileEventKind::Loaded => {
                out.events.push(PlaybackEvent::Started { load });
                // Attached by the caller, once its events are delivered.
                self.visualizer.started(load);
            }
            end @ FileEventKind::End { .. } => {
                self.entries.remove(&event.entry_id);
                self.deliver_end(out, load, end);
            }
        }
    }

    fn deliver_end(&mut self, out: &mut Output, load: LoadId, end: FileEventKind) {
        let FileEventKind::End { reason, file_error } = end else {
            return;
        };
        self.position.clear();
        let failed = reason == EndFileReason::Error;
        let reason = match reason {
            EndFileReason::Eof => EndReason::Eof,
            EndFileReason::Error => {
                EndReason::Error(file_error.unwrap_or_else(|| "playback error".into()))
            }
            // The file expanded into other entries (playlist or
            // directory); it is not a playable track.
            EndFileReason::Redirect => EndReason::Error(NOT_AUDIO.into()),
            EndFileReason::Stop | EndFileReason::Quit | EndFileReason::Unknown => {
                EndReason::Stopped
            }
        };
        out.events.push(PlaybackEvent::TrackEnded { load, reason });
        if self.latest == Some(load) {
            // Nothing can follow it any more: the player loads what comes
            // next, and a `SetNext` after it is stale.
            self.set_latest(None);
        }
        if self.visualizer.ended(load, failed) {
            self.visualizer_detach(out);
            out.events.push(PlaybackEvent::VisualizerUnavailable);
        }
    }

    fn on_property(&mut self, out: &mut Output, name: &str, data: Option<Value>, now: Instant) {
        match name {
            "time-pos" => {
                let raw = data.as_ref().and_then(Value::as_f64);
                // The first position above zero of an entry mpv advanced to
                // is when it is heard (see the module docs).
                if raw.is_some_and(|secs| secs > 0.0)
                    && let Some(unheard) = self.unheard.filter(|u| u.loaded)
                {
                    self.unheard = None;
                    self.heard(out, unheard.load);
                }
                if let Some(position) = data.and_then(|v| seconds(&v)) {
                    self.visualizer.position(position.as_secs_f64(), now);
                    if let Some(position) = self.position.offer(position, now) {
                        out.events.push(PlaybackEvent::Position(position));
                    }
                }
            }
            SAMPLE_RATE_PROPERTY => self
                .visualizer
                .set_sample_rate(data.and_then(|v| v.as_f64())),
            "duration" => out
                .events
                .push(PlaybackEvent::Duration(data.and_then(|v| seconds(&v)))),
            "pause" => {
                if let Some(paused) = data.and_then(|v| v.as_bool()) {
                    out.events.push(PlaybackEvent::Paused(paused));
                    self.visualizer.set_paused(paused, now);
                    self.visualizer_attach(out);
                }
            }
            "volume" => {
                if let Some(volume) = data.and_then(|v| v.as_f64()).filter(|v| v.is_finite()) {
                    out.events
                        .push(PlaybackEvent::Volume(volume.round().clamp(0.0, 100.0) as u8));
                }
            }
            _ => {}
        }
    }
}

/// The `playlist_entry_id` of a `loadfile` reply's data.
fn entry_id(data: Option<&Value>) -> Option<i64> {
    data?.get("playlist_entry_id")?.as_i64()
}

/// Converts an untrusted mpv number of seconds. Negative values clamp to
/// zero; NaN, infinities and values too large for a `Duration` are `None`.
fn seconds(value: &Value) -> Option<Duration> {
    let secs = value.as_f64().filter(|s| s.is_finite())?;
    Duration::try_from_secs_f64(secs.max(0.0)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn l(n: u64) -> LoadId {
        LoadId(n)
    }

    fn load(state: &mut MpvState, n: u64, path: &str) -> Output {
        state.handle_command(PlaybackCommand::Load {
            load: l(n),
            target: PlaybackTarget::File(path.into()),
        })
    }

    fn line(state: &mut MpvState, text: &str, now: Instant) -> Output {
        state.handle_line(text, now).expect("valid line")
    }

    fn reply_ok(state: &mut MpvState, id: u64, data: Value) -> Output {
        state.handle_message(
            MpvMessage::Reply {
                request_id: id,
                result: Ok(Some(data)),
            },
            Instant::now(),
        )
    }

    fn reply_err(state: &mut MpvState, id: u64, error: &str) -> Output {
        state.handle_message(
            MpvMessage::Reply {
                request_id: id,
                result: Err(error.into()),
            },
            Instant::now(),
        )
    }

    fn ev(state: &mut MpvState, text: &str) -> Vec<PlaybackEvent> {
        line(state, text, Instant::now()).events
    }

    fn start_file(id: i64) -> String {
        format!(r#"{{"event":"start-file","playlist_entry_id":{id}}}"#)
    }

    const FILE_LOADED: &str = r#"{"event":"file-loaded"}"#;

    fn end_file(id: i64, reason: &str) -> String {
        format!(r#"{{"event":"end-file","reason":"{reason}","playlist_entry_id":{id}}}"#)
    }

    /// The request id of the `loadfile … replace` in a load's output.
    fn loadfile_id(out: &Output) -> u64 {
        out.requests
            .iter()
            .find(|r| {
                matches!(
                    r.command,
                    MpvCommand::LoadFile { .. } | MpvCommand::LoadStream { .. }
                )
            })
            .map(|r| r.request_id)
            .expect("loadfile sent")
    }

    /// mpv's success reply to `loadfile` request `id`, naming `entry`.
    fn map(state: &mut MpvState, id: u64, entry: i64) -> Output {
        reply_ok(state, id, json!({ "playlist_entry_id": entry }))
    }

    fn loadfile_paths(out: &Output) -> Vec<String> {
        out.requests
            .iter()
            .filter_map(|r| match &r.command {
                MpvCommand::LoadFile { path } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn startup_observes_properties() {
        let mut s = MpvState::new();
        let out = s.startup();
        let names: Vec<_> = out
            .requests
            .iter()
            .map(|r| match &r.command {
                MpvCommand::ObserveProperty { name, .. } => *name,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(names, OBSERVED_PROPERTIES);
        for r in &out.requests {
            assert!(
                reply_ok(&mut s, r.request_id, Value::Null)
                    .events
                    .is_empty()
            );
        }
    }

    #[test]
    fn load_sends_unpause_then_loadfile() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        let commands: Vec<_> = out.requests.iter().map(|r| r.command.clone()).collect();
        assert_eq!(
            commands,
            [
                MpvCommand::SetProperty {
                    name: "pause",
                    value: false.into()
                },
                MpvCommand::LoadFile {
                    path: "/m/a.flac".into()
                },
            ]
        );
        let ids: Vec<_> = out.requests.iter().map(|r| r.request_id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert!(s.load_in_flight());
        assert!(out.events.is_empty());
    }

    #[test]
    fn replies_are_correlated_by_request_id() {
        let mut s = MpvState::new();
        let pause = s.handle_command(PlaybackCommand::SetPause(true));
        let out = load(&mut s, 1, "/m/a.flac");
        let entry_q = loadfile_id(&out);
        // Replies out of order: the entry-id reply is still matched to load 1.
        assert!(map(&mut s, entry_q, 5).events.is_empty());
        let failed = reply_err(&mut s, pause.requests[0].request_id, "property unavailable");
        assert_eq!(
            failed.events,
            [PlaybackEvent::Error(
                "Could not set pause: property unavailable".into()
            )]
        );
        // `Started` means the file is loaded, not merely opening.
        assert!(ev(&mut s, &start_file(5)).is_empty());
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(1) }]
        );
        // A reply for an unknown request id is ignored.
        assert!(reply_ok(&mut s, 9999, json!(1)).events.is_empty());
    }

    #[test]
    fn entry_id_mapping() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        // The reply to `loadfile` names the new entry.
        map(&mut s, loadfile_id(&out), 7);
        assert!(!s.load_in_flight());
        assert!(s.lookahead_enabled());
        // `Started` means the file is loaded, not merely opening.
        assert!(ev(&mut s, &start_file(7)).is_empty());
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(1) }]
        );
        assert_eq!(
            ev(&mut s, &end_file(7, "eof")),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
        // Mapping is gone after end-file.
        assert!(ev(&mut s, &end_file(7, "eof")).is_empty());
    }

    #[test]
    fn end_file_reasons_map_to_end_reasons() {
        for (reason, file_error, expected) in [
            ("eof", None, EndReason::Eof),
            (
                "error",
                Some("loading failed"),
                EndReason::Error("loading failed".into()),
            ),
            ("error", None, EndReason::Error("playback error".into())),
            ("stop", None, EndReason::Stopped),
            ("quit", None, EndReason::Stopped),
            ("redirect", None, EndReason::Error(NOT_AUDIO.into())),
            ("unknown", None, EndReason::Stopped),
        ] {
            let mut s = MpvState::new();
            let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
            map(&mut s, entry_q, 1);
            let mut text = json!({"event": "end-file", "reason": reason, "playlist_entry_id": 1});
            if let Some(e) = file_error {
                text["file_error"] = e.into();
            }
            assert_eq!(
                ev(&mut s, &text.to_string()),
                [PlaybackEvent::TrackEnded {
                    load: l(1),
                    reason: expected
                }]
            );
        }
    }

    #[test]
    fn events_before_the_loadfile_reply_are_buffered_and_replayed() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        assert!(ev(&mut s, &start_file(3)).is_empty());
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        assert!(ev(&mut s, &end_file(3, "eof")).is_empty());
        let out = map(&mut s, entry_q, 3);
        assert_eq!(
            out.events,
            [
                PlaybackEvent::Started { load: l(1) },
                PlaybackEvent::TrackEnded {
                    load: l(1),
                    reason: EndReason::Eof
                }
            ]
        );
    }

    #[test]
    fn unknown_entry_ids_are_dropped() {
        let mut s = MpvState::new();
        // Nothing pending: dropped immediately, and a stray end-file stops
        // nothing.
        let out = line(&mut s, &end_file(42, "eof"), Instant::now());
        assert_eq!(out, Output::default());
        // Pending: buffered, then dropped when the mapping is for another id.
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        assert!(ev(&mut s, &end_file(41, "eof")).is_empty());
        let out = map(&mut s, entry_q, 43);
        assert!(out.events.is_empty());
        // Events without an entry id are ignored.
        assert!(ev(&mut s, r#"{"event":"end-file","reason":"eof"}"#).is_empty());
    }

    #[test]
    fn buffer_is_bounded_and_drops_newest() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        // The real events arrive first and must survive a later flood.
        assert!(ev(&mut s, &start_file(2)).is_empty());
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        for i in 0..2 * MAX_BUFFERED_EVENTS {
            ev(&mut s, &end_file(100 + i as i64, "stop"));
        }
        assert_eq!(s.buffered.len(), MAX_BUFFERED_EVENTS);
        let out = map(&mut s, entry_q, 2);
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(1) }]);
        assert!(s.buffered.is_empty());
    }

    #[test]
    fn ids_not_newer_than_the_highest_mapped_id_are_never_buffered() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        map(&mut s, entry_q, 5);
        ev(&mut s, &end_file(5, "eof"));
        load(&mut s, 2, "/m/b.flac");
        assert!(ev(&mut s, &end_file(3, "eof")).is_empty());
        assert!(ev(&mut s, &end_file(5, "eof")).is_empty());
        assert!(s.buffered.is_empty());
        assert!(ev(&mut s, &end_file(6, "eof")).is_empty());
        assert_eq!(s.buffered.len(), 1);
    }

    #[test]
    fn file_loaded_is_attributed_to_the_opening_entry() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        map(&mut s, entry_q, 1);
        // No start-file yet: nothing to attribute.
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        ev(&mut s, &start_file(1));
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(1) }]
        );
        // A file that fails to open: start-file, end-file{error}, no Started.
        let entry_q = loadfile_id(&load(&mut s, 2, "/m/b.flac"));
        map(&mut s, entry_q, 2);
        ev(&mut s, &end_file(1, "stop"));
        ev(&mut s, &start_file(2));
        let ended = ev(&mut s, &end_file(2, "error"));
        assert!(matches!(
            ended.as_slice(),
            [PlaybackEvent::TrackEnded {
                reason: EndReason::Error(_),
                ..
            }]
        ));
        // A late file-loaded after end-file is not attributed to anything.
        assert!(ev(&mut s, FILE_LOADED).is_empty());
    }

    #[test]
    fn redirect_of_a_mapped_entry_is_an_error() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/evil.mp3"));
        map(&mut s, entry_q, 1);
        ev(&mut s, &start_file(1));
        // Shape observed from mpv 0.41 for a content-detected playlist.
        let redirect = r#"{"event":"end-file","reason":"redirect","playlist_entry_id":1,"playlist_insert_id":2,"playlist_insert_num_entries":2}"#;
        assert_eq!(
            ev(&mut s, redirect),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Error(NOT_AUDIO.into())
            }]
        );
    }

    #[test]
    fn unrequested_start_file_is_stopped() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/evil.mp3"));
        map(&mut s, entry_q, 1);
        ev(&mut s, &start_file(1));
        ev(&mut s, &end_file(1, "redirect"));
        // mpv moves on to an inserted entry Coda never requested.
        let out = line(&mut s, &start_file(2), Instant::now());
        assert!(out.events.is_empty());
        assert_eq!(out.requests.len(), 1);
        assert_eq!(out.requests[0].command, MpvCommand::Stop);
        // Its file-loaded and end-file produce nothing further.
        assert_eq!(line(&mut s, FILE_LOADED, Instant::now()), Output::default());
        assert_eq!(
            line(&mut s, &end_file(2, "stop"), Instant::now()),
            Output::default()
        );
    }

    #[test]
    fn unmapped_start_file_while_a_load_is_in_flight_is_not_stopped() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        map(&mut s, entry_q, 3);
        load(&mut s, 2, "/m/b.flac");
        // An old id while a load is pending: dropped without a stop, which
        // would otherwise also stop the load in flight.
        let out = line(&mut s, &start_file(2), Instant::now());
        assert_eq!(out, Output::default());
    }

    #[test]
    fn queued_loads_collapse_to_the_latest() {
        let mut s = MpvState::new();
        let first = load(&mut s, 1, "/m/a.flac");
        let entry_q = loadfile_id(&first);
        for (n, p) in [(2, "/m/b.flac"), (3, "/m/c.flac"), (4, "/m/d.flac")] {
            assert_eq!(load(&mut s, n, p), Output::default());
        }
        let out = map(&mut s, entry_q, 1);
        assert_eq!(loadfile_paths(&out), ["/m/d.flac"]);
        assert!(s.load_in_flight());
        let entry_q = loadfile_id(&out);
        map(&mut s, entry_q, 2);
        // `Started` means the file is loaded, not merely opening.
        assert!(ev(&mut s, &start_file(2)).is_empty());
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(4) }]
        );
    }

    #[test]
    fn failed_loadfile_ends_the_load_with_error() {
        let mut s = MpvState::new();
        let file_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        load(&mut s, 2, "/m/b.flac");
        let out = reply_err(&mut s, file_q, "invalid parameter");
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Error("mpv could not load the file: invalid parameter".into())
            }]
        );
        // Nothing is mapped, and the queued load proceeds at once.
        assert_eq!(loadfile_paths(&out), ["/m/b.flac"]);
        assert!(ev(&mut s, &end_file(9, "eof")).is_empty());
    }

    #[test]
    fn a_loadfile_reply_without_an_entry_id_falls_back_to_querying_it() {
        // An mpv that does not name the new entry (none tested does) loads
        // as before M8, and the lookahead is off for the session.
        let mut s = MpvState::new();
        let file_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        let out = reply_ok(&mut s, file_q, Value::Null);
        assert!(out.events.is_empty());
        let [query] = out.requests.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(
            query.command,
            MpvCommand::GetProperty {
                name: ENTRY_ID_PROPERTY
            }
        );
        assert!(s.load_in_flight());
        assert!(!s.lookahead_enabled());
        // Events wait for the mapping, as before.
        assert!(ev(&mut s, &start_file(4)).is_empty());
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        let out = reply_ok(&mut s, query.request_id, json!(4));
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(1) }]);
        // A next entry is never sent.
        let out = s.handle_command(PlaybackCommand::SetNext {
            after: l(1),
            next: Some((l(2), PlaybackTarget::File("/m/b.flac".into()))),
        });
        assert_eq!(out, Output::default());
        // At the end, an ordinary EOF.
        assert_eq!(
            ev(&mut s, &end_file(4, "eof")),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
    }

    #[test]
    fn failed_entry_id_query_ends_the_load_with_error() {
        let mut s = MpvState::new();
        let file_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        let out = reply_ok(&mut s, file_q, json!({}));
        let entry_q = out.requests[0].request_id;
        let out = reply_err(&mut s, entry_q, "property not found");
        assert!(matches!(
            out.events.as_slice(),
            [PlaybackEvent::TrackEnded {
                reason: EndReason::Error(_),
                ..
            }]
        ));
        assert!(!s.load_in_flight());
    }

    #[test]
    fn missing_file_never_reaches_mpv_and_supersedes_queue() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        load(&mut s, 2, "/m/b.flac");
        let out = s.load_missing(l(3));
        assert!(out.requests.is_empty());
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(3),
                reason: EndReason::Error(FILE_NOT_FOUND.into())
            }]
        );
        // The queued load 2 was superseded.
        assert!(loadfile_paths(&map(&mut s, entry_q, 1)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_is_rejected_without_contacting_mpv() {
        use std::os::unix::ffi::OsStrExt;
        let mut s = MpvState::new();
        let path = std::ffi::OsStr::from_bytes(b"/m/\xff.flac");
        let out = s.handle_command(PlaybackCommand::Load {
            load: l(1),
            target: PlaybackTarget::File(path.into()),
        });
        assert!(out.requests.is_empty());
        assert!(matches!(
            out.events.as_slice(),
            [PlaybackEvent::TrackEnded {
                reason: EndReason::Error(_),
                ..
            }]
        ));
    }

    #[test]
    fn stream_load_sends_loadfile_with_the_url() {
        let mut s = MpvState::new();
        let url = url::Url::parse("http://127.0.0.1:1/rest/stream?id=x").unwrap();
        let stream = crate::playback::StreamUrl::new(url).unwrap();
        let out = s.handle_command(PlaybackCommand::Load {
            load: l(1),
            target: PlaybackTarget::Stream(stream.clone()),
        });
        assert!(out.events.is_empty());
        assert!(out.requests.iter().any(|r| r.command
            == MpvCommand::LoadStream {
                url: stream.clone()
            }));
        assert!(loadfile_id(&out) > 0);
        assert_eq!(out.requests.len(), 2, "no entry id query");
    }

    #[test]
    fn stop_cancels_queued_load() {
        let mut s = MpvState::new();
        let entry_q = loadfile_id(&load(&mut s, 1, "/m/a.flac"));
        load(&mut s, 2, "/m/b.flac");
        let out = s.handle_command(PlaybackCommand::Stop);
        assert_eq!(out.requests.len(), 1);
        assert_eq!(out.requests[0].command, MpvCommand::Stop);
        assert!(loadfile_paths(&map(&mut s, entry_q, 1)).is_empty());
    }

    #[test]
    fn simple_commands() {
        let mut s = MpvState::new();
        let cases = [
            (
                PlaybackCommand::SetPause(true),
                MpvCommand::SetProperty {
                    name: "pause",
                    value: true.into(),
                },
            ),
            (
                PlaybackCommand::SetVolume(55),
                MpvCommand::SetProperty {
                    name: "volume",
                    value: 55.into(),
                },
            ),
            (
                PlaybackCommand::SeekRelative(-5.0),
                MpvCommand::Seek {
                    seconds: -5.0,
                    mode: SeekMode::Relative,
                },
            ),
            (
                PlaybackCommand::SeekAbsolute(0.0),
                MpvCommand::Seek {
                    seconds: 0.0,
                    mode: SeekMode::Absolute,
                },
            ),
            (PlaybackCommand::Shutdown, MpvCommand::Quit),
        ];
        for (command, expected) in cases {
            let out = s.handle_command(command);
            assert_eq!(out.requests.len(), 1);
            assert_eq!(out.requests[0].command, expected);
        }
        assert!(
            s.handle_command(PlaybackCommand::SeekRelative(f64::NAN))
                .requests
                .is_empty()
        );
    }

    #[test]
    fn property_changes_become_events() {
        let mut s = MpvState::new();
        let prop = |name: &str, data: Option<Value>| {
            let mut v = json!({"event": "property-change", "id": 1, "name": name});
            if let Some(d) = data {
                v["data"] = d;
            }
            v.to_string()
        };
        assert_eq!(
            ev(&mut s, &prop("pause", Some(json!(true)))),
            [PlaybackEvent::Paused(true)]
        );
        assert_eq!(
            ev(&mut s, &prop("volume", Some(json!(54.6)))),
            [PlaybackEvent::Volume(55)]
        );
        assert_eq!(
            ev(&mut s, &prop("volume", Some(json!(130.0)))),
            [PlaybackEvent::Volume(100)]
        );
        assert_eq!(
            ev(&mut s, &prop("duration", Some(json!(3.0)))),
            [PlaybackEvent::Duration(Some(Duration::from_secs(3)))]
        );
        assert_eq!(
            ev(&mut s, &prop("duration", None)),
            [PlaybackEvent::Duration(None)]
        );
        assert!(ev(&mut s, &prop("time-pos", None)).is_empty());
        assert!(ev(&mut s, &prop("something-else", Some(json!(1)))).is_empty());
    }

    #[test]
    fn untrusted_numbers_never_panic() {
        let mut s = MpvState::new();
        for data in [
            json!(1e300),
            json!(f64::MAX),
            json!(-1e300),
            json!(-5.0),
            json!("3.0"),
            json!(null),
            json!([1]),
        ] {
            for name in ["time-pos", "duration", "volume"] {
                let text =
                    json!({"event":"property-change","id":1,"name":name,"data":data}).to_string();
                let _ = ev(&mut s, &text);
            }
        }
        assert_eq!(seconds(&json!(1e300)), None);
        assert_eq!(seconds(&json!(f64::MAX)), None);
        assert_eq!(seconds(&json!(-5.0)), Some(Duration::ZERO));
        assert_eq!(seconds(&json!(2.5)), Some(Duration::from_millis(2500)));
        assert_eq!(seconds(&json!("x")), None);
        let huge = json!({"event":"property-change","id":2,"name":"duration","data":1e300});
        assert_eq!(
            ev(&mut s, &huge.to_string()),
            [PlaybackEvent::Duration(None)]
        );
    }

    #[test]
    fn positions_are_coalesced_to_four_per_second() {
        let mut s = MpvState::new();
        let t0 = Instant::now();
        let pos = |secs: f64| {
            json!({"event":"property-change","id":1,"name":"time-pos","data":secs}).to_string()
        };
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        assert_eq!(
            line(&mut s, &pos(0.0), at(0)).events,
            [PlaybackEvent::Position(Duration::ZERO)]
        );
        assert!(line(&mut s, &pos(0.1), at(100)).events.is_empty());
        assert!(line(&mut s, &pos(0.2), at(200)).events.is_empty());
        assert!(s.tick(at(200)).events.is_empty(), "not due yet");
        assert_eq!(
            s.tick(at(250)).events,
            [PlaybackEvent::Position(Duration::from_secs_f64(0.2))]
        );
        assert!(s.tick(at(600)).events.is_empty(), "nothing pending");
        assert_eq!(
            line(&mut s, &pos(0.6), at(600)).events,
            [PlaybackEvent::Position(Duration::from_secs_f64(0.6))]
        );
        // A pending position is dropped at a track boundary.
        assert!(line(&mut s, &pos(0.7), at(700)).events.is_empty());
        load(&mut s, 1, "/m/a.flac");
        assert!(s.tick(at(2000)).events.is_empty());
    }

    #[test]
    fn rejected_volume_is_a_structured_event() {
        let mut s = MpvState::new();
        let out = s.handle_command(PlaybackCommand::SetVolume(130));
        assert_eq!(
            out.requests[0].command,
            MpvCommand::SetProperty {
                name: "volume",
                value: 100.into()
            }
        );
        // The event names the clamped value that was actually requested.
        assert_eq!(
            reply_err(&mut s, out.requests[0].request_id, "property unavailable").events,
            [PlaybackEvent::VolumeRejected {
                volume: 100,
                reason: "property unavailable".into()
            }]
        );
        let out = s.handle_command(PlaybackCommand::SetVolume(40));
        assert!(
            reply_ok(&mut s, out.requests[0].request_id, Value::Null)
                .events
                .is_empty(),
            "success is confirmed by the property echo, not the reply"
        );
    }

    /// Checks the watchdog every `step` from `from` (exclusive) to `to`
    /// (inclusive), returning the first overdue report and when it came.
    fn watch(
        s: &mut MpvState,
        from: Instant,
        to: Instant,
        step: Duration,
    ) -> Option<(Instant, (u64, Duration))> {
        let mut now = from;
        while now < to {
            now = (now + step).min(to);
            if let Some(overdue) = s.overdue_reply(now) {
                return Some((now, overdue));
            }
        }
        None
    }

    #[test]
    fn unanswered_request_is_overdue_after_the_timeout() {
        let t0 = Instant::now();
        let secs = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut s = MpvState::with_reply_timeout(Duration::from_secs(10));
        assert_eq!(s.overdue_reply(t0), None);
        let out = s.startup();
        s.requests_sent(&out.requests, t0);
        // Startup replies arrive; a later pause request never gets one.
        for r in &out.requests {
            reply_ok(&mut s, r.request_id, Value::Null);
        }
        assert_eq!(watch(&mut s, t0, secs(30.0), POSITION_INTERVAL), None);
        let pause = s.handle_command(PlaybackCommand::SetPause(true));
        s.requests_sent(&pause.requests, secs(30.0));
        let (at, (id, waited)) =
            watch(&mut s, secs(30.0), secs(60.0), POSITION_INTERVAL).expect("overdue");
        assert_eq!(id, pause.requests[0].request_id);
        assert_eq!(at, secs(40.0));
        assert_eq!(waited, Duration::from_secs(10));
    }

    #[test]
    fn answered_requests_never_become_overdue_and_the_oldest_is_reported() {
        let t0 = Instant::now();
        let secs = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut s = MpvState::with_reply_timeout(Duration::from_secs(10));
        s.overdue_reply(t0);
        let first = load(&mut s, 1, "/m/a.flac");
        s.requests_sent(&first.requests, t0);
        assert_eq!(watch(&mut s, t0, secs(5.0), POSITION_INTERVAL), None);
        let volume = s.handle_command(PlaybackCommand::SetVolume(50));
        s.requests_sent(&volume.requests, secs(5.0));
        // The load is answered in full; only the volume request is left.
        reply_ok(&mut s, first.requests[0].request_id, Value::Null);
        map(&mut s, loadfile_id(&first), 1);
        // A reply for an unknown id changes nothing.
        reply_ok(&mut s, 9999, Value::Null);
        assert_eq!(
            watch(&mut s, secs(5.0), secs(14.9), POSITION_INTERVAL),
            None
        );
        let (at, (id, _)) =
            watch(&mut s, secs(14.9), secs(20.0), Duration::from_millis(100)).expect("overdue");
        assert_eq!(id, volume.requests[0].request_id);
        assert_eq!(at, secs(15.0));

        // Two unanswered requests: the oldest is reported.
        let mut s = MpvState::with_reply_timeout(Duration::from_secs(10));
        s.overdue_reply(t0);
        let a = s.handle_command(PlaybackCommand::SetPause(true));
        s.requests_sent(&a.requests, t0);
        s.overdue_reply(secs(1.0));
        let b = s.handle_command(PlaybackCommand::SetPause(false));
        s.requests_sent(&b.requests, secs(1.0));
        let (_, (id, _)) = watch(&mut s, secs(1.0), secs(20.0), POSITION_INTERVAL).unwrap();
        assert_eq!(id, a.requests[0].request_id);
    }

    #[test]
    fn time_not_spent_reading_from_mpv_is_excused() {
        // The driver was blocked for 30 s (a stalled event consumer, or the
        // process was suspended): mpv could not have replied meanwhile.
        let t0 = Instant::now();
        let secs = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut s = MpvState::with_reply_timeout(Duration::from_secs(10));
        s.overdue_reply(t0);
        let pause = s.handle_command(PlaybackCommand::SetPause(true));
        s.requests_sent(&pause.requests, t0);
        assert_eq!(watch(&mut s, t0, secs(2.0), POSITION_INTERVAL), None);
        assert_eq!(s.overdue_reply(secs(32.0)), None, "the gap is excused");
        // Only listening time counts: 2 s before the gap, 8 s after it.
        assert_eq!(
            watch(&mut s, secs(32.0), secs(39.9), POSITION_INTERVAL),
            None
        );
        let (at, _) =
            watch(&mut s, secs(39.9), secs(45.0), Duration::from_millis(100)).expect("overdue");
        assert_eq!(at, secs(40.0));

        // Gaps up to `MAX_CHECK_GAP` are normal scheduling and count.
        let mut s = MpvState::with_reply_timeout(Duration::from_secs(10));
        s.overdue_reply(t0);
        let stop = s.handle_command(PlaybackCommand::Stop);
        s.requests_sent(&stop.requests, t0);
        let (at, _) = watch(&mut s, t0, secs(20.0), MAX_CHECK_GAP).expect("overdue");
        assert_eq!(at, secs(10.0));
    }

    /// Load 1 mapped to entry 1 and started; returns the `file-loaded`
    /// output.
    fn started(s: &mut MpvState) -> Output {
        let out = load(s, 1, "/m/a.flac");
        map(s, loadfile_id(&out), 1);
        ev(s, &start_file(1));
        line(s, FILE_LOADED, Instant::now())
    }

    fn af_requests(out: &Output) -> Vec<MpvCommand> {
        out.requests
            .iter()
            .map(|r| r.command.clone())
            .filter(|c| matches!(c, MpvCommand::AfAdd { .. } | MpvCommand::AfRemove { .. }))
            .collect()
    }

    fn request_id(out: &Output, pred: fn(&MpvCommand) -> bool) -> u64 {
        out.requests
            .iter()
            .find(|r| pred(&r.command))
            .expect("request present")
            .request_id
    }

    const ADD: fn(&MpvCommand) -> bool = |c| matches!(c, MpvCommand::AfAdd { .. });
    const REMOVE: MpvCommand = MpvCommand::AfRemove {
        labels: visualizer::GRAPH_LABELS,
    };

    fn add() -> MpvCommand {
        MpvCommand::AfAdd {
            graph: visualizer::GRAPH.into(),
        }
    }

    fn property(name: &str, data: Value) -> String {
        json!({"event": "property-change", "id": 1, "name": name, "data": data}).to_string()
    }

    #[test]
    fn the_visualizer_attaches_lazily_while_playing() {
        let mut s = MpvState::new();
        // Wanted while idle: nothing is added and nothing polled.
        let out = s.handle_command(PlaybackCommand::SetVisualizer(true));
        assert_eq!(out, Output::default());
        assert_eq!(s.visualizer_deadline(Instant::now()), None);
        // The track starts: added once.
        let out = started(&mut s);
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(1) }]);
        assert_eq!(af_requests(&out), [add()]);
        // Not polled until mpv accepts it.
        assert_eq!(s.visualizer_deadline(Instant::now()), None);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        let now = Instant::now();
        assert_eq!(s.visualizer_deadline(now), Some(now));
        let tick = s.visualizer_tick(now);
        assert_eq!(
            tick.requests
                .iter()
                .map(|r| r.command.clone())
                .collect::<Vec<_>>(),
            [MpvCommand::GetProperty {
                name: visualizer::METADATA_PROPERTY
            }]
        );
        // A pause stops the polls; nothing is added twice.
        let out = line(&mut s, &property("pause", json!(true)), now);
        assert!(af_requests(&out).is_empty());
        assert_eq!(s.visualizer_deadline(now), None);
        assert_eq!(s.visualizer_tick(now), Output::default());
    }

    #[test]
    fn the_visualizer_is_not_added_while_paused_and_is_on_resume() {
        let mut s = MpvState::new();
        let out = started(&mut s);
        assert!(af_requests(&out).is_empty(), "not wanted");
        ev(&mut s, &property("pause", json!(true)));
        let out = s.handle_command(PlaybackCommand::SetVisualizer(true));
        assert!(af_requests(&out).is_empty(), "paused");
        let out = line(&mut s, &property("pause", json!(false)), Instant::now());
        assert_eq!(af_requests(&out), [add()]);
    }

    #[test]
    fn a_hidden_visualizer_is_removed_before_the_next_load_or_stop() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        // Hidden: no removal mid-track, and no polls.
        let out = s.handle_command(PlaybackCommand::SetVisualizer(false));
        assert_eq!(out, Output::default());
        assert_eq!(s.visualizer_deadline(Instant::now()), None);
        let out = s.handle_command(PlaybackCommand::SeekRelative(5.0));
        assert!(af_requests(&out).is_empty());
        // The next load removes it before anything else.
        let out = load(&mut s, 2, "/m/b.flac");
        assert_eq!(out.requests[0].command, REMOVE);
        assert_eq!(loadfile_paths(&out), ["/m/b.flac"]);
        // Removed once only.
        let out = s.handle_command(PlaybackCommand::Stop);
        assert_eq!(af_requests(&out), []);

        // Visible at a stop: kept.
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        let out = s.handle_command(PlaybackCommand::Stop);
        assert_eq!(af_requests(&out), []);
        // Hidden later: removed at the following stop.
        s.handle_command(PlaybackCommand::SetVisualizer(false));
        let out = s.handle_command(PlaybackCommand::Stop);
        assert_eq!(out.requests[0].command, REMOVE);
    }

    #[test]
    fn a_refused_graph_is_reported_once_and_playback_goes_on() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        let failed = reply_err(&mut s, request_id(&out, ADD), "error running command");
        assert_eq!(failed.events, [PlaybackEvent::VisualizerUnavailable]);
        assert!(failed.requests.is_empty());
        // Never retried, never polled; the track still ends normally.
        s.handle_command(PlaybackCommand::SetVisualizer(false));
        assert_eq!(
            s.handle_command(PlaybackCommand::SetVisualizer(true)),
            Output::default()
        );
        assert_eq!(s.visualizer_deadline(Instant::now()), None);
        assert_eq!(
            ev(&mut s, &end_file(1, "eof")),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
    }

    #[test]
    fn a_track_failing_on_an_unproven_graph_removes_it() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        let out = line(&mut s, &end_file(1, "error"), Instant::now());
        assert_eq!(af_requests(&out), [REMOVE]);
        assert!(out.events.contains(&PlaybackEvent::VisualizerUnavailable));
        assert!(matches!(
            out.events[0],
            PlaybackEvent::TrackEnded {
                reason: EndReason::Error(_),
                ..
            }
        ));
    }

    #[test]
    fn a_load_superseded_while_its_events_were_buffered_is_not_attached() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let first = load(&mut s, 1, "/m/a.flac");
        let entry_q = loadfile_id(&first);
        load(&mut s, 2, "/m/b.flac");
        // Load 1's file events arrive before its entry id.
        ev(&mut s, &start_file(1));
        ev(&mut s, FILE_LOADED);
        let out = map(&mut s, entry_q, 1);
        assert!(out.events.contains(&PlaybackEvent::Started { load: l(1) }));
        assert_eq!(loadfile_paths(&out), ["/m/b.flac"]);
        assert!(af_requests(&out).is_empty(), "{out:?}");
        // Load 2 starts: added once.
        let entry_q = loadfile_id(&out);
        map(&mut s, entry_q, 2);
        ev(&mut s, &start_file(2));
        let out = line(&mut s, FILE_LOADED, Instant::now());
        assert_eq!(af_requests(&out), [add()]);
    }

    #[test]
    fn only_mpvs_seek_event_holds_readings_back() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        let now = Instant::now();
        line(&mut s, r#"{"event":"playback-restart"}"#, now);
        line(&mut s, &property("time-pos", json!(1.0)), now);
        s.handle_command(PlaybackCommand::SeekRelative(5.0));
        assert!(!s.visualizer.restarting_for_tests(), "a sent seek alone");
        ev(&mut s, r#"{"event":"seek"}"#);
        assert!(s.visualizer.restarting_for_tests());
        ev(&mut s, r#"{"event":"playback-restart"}"#);
        assert!(!s.visualizer.restarting_for_tests());
    }

    #[test]
    fn readings_reach_the_boundary_aligned_and_coalesced() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        line(&mut s, r#"{"event":"playback-restart"}"#, at(0));
        line(&mut s, &property("time-pos", json!(1.0)), at(0));
        let reading = |t: f64, db: &str| {
            let mut map = serde_json::Map::new();
            map.insert("lavfi.astats.1.RMS_level".into(), json!(db));
            map.insert("lavfi.astats.2.RMS_level".into(), json!(db));
            let tc = format!("{:.6}", 20.0 * (t / 32.0).log10());
            map.insert("lavfi.astats.10.Peak_level".into(), json!(tc));
            Value::Object(map)
        };
        let poll = |s: &mut MpvState, now| {
            let out = s.visualizer_tick(now);
            (out.events, out.requests.first().map(|r| r.request_id))
        };
        let mut shown = Vec::new();
        // Three readings 50 ms apart, 0.25 s ahead of the position.
        for (i, ms) in [0u64, 50, 100].into_iter().enumerate() {
            let (events, id) = poll(&mut s, at(ms));
            shown.extend(events);
            let t = 1.25 + i as f64 * 0.05;
            let db = ["-60", "-30", "0"][i];
            s.handle_message(
                MpvMessage::Reply {
                    request_id: id.expect("poll sent"),
                    result: Ok(Some(reading(t, db))),
                },
                at(ms + 1),
            );
        }
        assert!(shown.is_empty(), "nothing is due yet: {shown:?}");
        // At 1.40 s all three are due; only the newest is sent.
        let (events, _) = poll(&mut s, at(400));
        let [PlaybackEvent::Visualizer { load, frame }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!(*load, l(1));
        assert_eq!(frame.levels, [1.0, 1.0]);
    }

    #[test]
    fn default_reply_timeout_is_ten_seconds() {
        let t0 = Instant::now();
        let mut s = MpvState::new();
        s.overdue_reply(t0);
        let startup = s.startup();
        s.requests_sent(&startup.requests, t0);
        let (at, _) =
            watch(&mut s, t0, t0 + Duration::from_secs(60), POSITION_INTERVAL).expect("overdue");
        assert_eq!(at, t0 + REPLY_TIMEOUT);
    }

    // ----- gapless lookahead (M8) -------------------------------------------

    fn commands(out: &Output) -> Vec<MpvCommand> {
        out.requests.iter().map(|r| r.command.clone()).collect()
    }

    fn set_next(s: &mut MpvState, after: u64, next: Option<(u64, &str)>) -> Output {
        s.handle_command(PlaybackCommand::SetNext {
            after: l(after),
            next: next.map(|(n, path)| (l(n), PlaybackTarget::File(path.into()))),
        })
    }

    fn append(path: &str) -> MpvCommand {
        MpvCommand::AppendFile { path: path.into() }
    }

    /// Answers every request of a set-next; the append (if any) names
    /// `entry`.
    fn ack_next(s: &mut MpvState, out: &Output, entry: i64) -> Output {
        let mut all = Output::default();
        for r in &out.requests {
            let reply = match r.command {
                MpvCommand::AppendFile { .. } | MpvCommand::AppendStream { .. } => {
                    map(s, r.request_id, entry)
                }
                _ => reply_ok(s, r.request_id, Value::Null),
            };
            all.requests.extend(reply.requests);
            all.events.extend(reply.events);
        }
        all
    }

    /// Load 1 (`/m/a.flac`) plays as entry 1; load 2 (`/m/b.flac`) is
    /// appended as entry 2 and acknowledged.
    fn playing_with_next(s: &mut MpvState) {
        let out = load(s, 1, "/m/a.flac");
        map(s, loadfile_id(&out), 1);
        ev(s, &start_file(1));
        ev(s, FILE_LOADED);
        let out = set_next(s, 1, Some((2, "/m/b.flac")));
        assert_eq!(
            commands(&out),
            [MpvCommand::PlaylistClear, append("/m/b.flac")]
        );
        assert!(ack_next(s, &out, 2).requests.is_empty());
    }

    fn switch_check(out: &Output) -> u64 {
        request_id(out, |c| {
            *c == MpvCommand::GetProperty {
                name: IDLE_PROPERTY,
            }
        })
    }

    fn time_pos(secs: f64) -> String {
        property("time-pos", json!(secs))
    }

    #[test]
    fn the_next_entry_waits_for_the_current_one_to_be_mapped() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        // Announced while the load is in flight: held back.
        assert_eq!(
            set_next(&mut s, 1, Some((2, "/m/b.flac"))),
            Output::default()
        );
        // Sent once the load's entry is known: clear, then append.
        let mapped = map(&mut s, loadfile_id(&out), 1);
        assert_eq!(
            commands(&mapped),
            [MpvCommand::PlaylistClear, append("/m/b.flac")]
        );
        // The append's reply maps the new entry.
        ack_next(&mut s, &mapped, 2);
        ev(&mut s, &start_file(1));
        ev(&mut s, FILE_LOADED);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.events.is_empty(), "waits for mpv's choice: {out:?}");
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
    }

    #[test]
    fn set_next_for_another_load_is_stale() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        assert_eq!(
            set_next(&mut s, 7, Some((8, "/m/b.flac"))),
            Output::default()
        );
        // A newer load in flight makes a next entry for the old one stale,
        // even after it is mapped.
        load(&mut s, 2, "/m/c.flac");
        assert_eq!(
            set_next(&mut s, 1, Some((3, "/m/b.flac"))),
            Output::default()
        );
        // And after a stop, nothing follows anything.
        s.handle_command(PlaybackCommand::Stop);
        assert_eq!(
            set_next(&mut s, 2, Some((3, "/m/b.flac"))),
            Output::default()
        );
    }

    #[test]
    fn set_next_changes_collapse_to_the_latest_and_repeat_nothing() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        // The same next again: nothing to do.
        assert_eq!(
            set_next(&mut s, 1, Some((2, "/m/b.flac"))),
            Output::default()
        );
        // Three changes while the first is in flight: only the last follows.
        let first = set_next(&mut s, 1, Some((3, "/m/c.flac")));
        assert_eq!(
            commands(&first),
            [MpvCommand::PlaylistClear, append("/m/c.flac")]
        );
        assert_eq!(set_next(&mut s, 1, None), Output::default());
        assert_eq!(
            set_next(&mut s, 1, Some((5, "/m/e.flac"))),
            Output::default()
        );
        let last = ack_next(&mut s, &first, 3);
        assert_eq!(
            commands(&last),
            [MpvCommand::PlaylistClear, append("/m/e.flac")]
        );
        ack_next(&mut s, &last, 4);
        // No next entry: a clear alone.
        let out = set_next(&mut s, 1, None);
        assert_eq!(commands(&out), [MpvCommand::PlaylistClear]);
        ack_next(&mut s, &out, 0);
        assert_eq!(set_next(&mut s, 1, None), Output::default());
        // Going back to a next already superseded sends it again (a fresh
        // entry; the old one is gone from mpv's playlist).
        let out = set_next(&mut s, 1, Some((5, "/m/e.flac")));
        assert_eq!(
            commands(&out),
            [MpvCommand::PlaylistClear, append("/m/e.flac")]
        );
    }

    #[test]
    fn a_gapless_change_is_reported_when_mpv_starts_the_next_entry() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.events.is_empty(), "{out:?}");
        let check = switch_check(&out);
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
        // mpv's answer to the check comes after; it changes nothing.
        let out = reply_ok(&mut s, check, json!(false));
        assert_eq!(out, Output::default());
        // `Started` waits until the new track is heard (below).
        assert!(ev(&mut s, FILE_LOADED).is_empty());
    }

    #[test]
    fn mpv_answering_the_check_first_still_waits_for_the_start() {
        // mpv writes the reply before events that happened while it ran.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        let out = reply_ok(&mut s, switch_check(&out), json!(false));
        assert_eq!(out, Output::default());
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
    }

    #[test]
    fn an_idle_mpv_ends_the_track_as_before() {
        // The clear removed the next entry before the end, although its
        // reply came later: mpv stopped.
        for answer in [json!(true), json!("garbage"), Value::Null] {
            let mut s = MpvState::new();
            playing_with_next(&mut s);
            let out = line(&mut s, &end_file(1, "eof"), Instant::now());
            let out = reply_ok(&mut s, switch_check(&out), answer.clone());
            assert_eq!(
                out.events,
                [PlaybackEvent::TrackEnded {
                    load: l(1),
                    reason: EndReason::Eof
                }],
                "{answer}"
            );
        }
        // An error answer: the same.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        let out = reply_err(&mut s, switch_check(&out), "property unavailable");
        assert!(matches!(
            out.events.as_slice(),
            [PlaybackEvent::TrackEnded {
                reason: EndReason::Eof,
                ..
            }]
        ));
    }

    #[test]
    fn without_a_possible_successor_the_end_is_reported_at_once() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        ev(&mut s, &start_file(1));
        ev(&mut s, FILE_LOADED);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.requests.is_empty());
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
        // An error end never waits: mpv's choice is stale for the player.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "error"), Instant::now());
        assert!(matches!(
            out.events.as_slice(),
            [PlaybackEvent::TrackEnded {
                reason: EndReason::Error(_),
                ..
            }]
        ));
        // mpv goes on to the appended entry; it is reported as an ordinary
        // (stale) entry: started at `file-loaded`, no `Advanced`.
        assert!(ev(&mut s, &start_file(2)).is_empty());
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(2) }]
        );
    }

    #[test]
    fn the_successor_is_whatever_mpv_starts_even_a_superseded_one() {
        // The change to C was answered before A's end-file but ran after
        // mpv had chosen B.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = set_next(&mut s, 1, Some((3, "/m/c.flac")));
        ack_next(&mut s, &out, 3);
        line(&mut s, &end_file(1, "eof"), Instant::now());
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
        // The clear kept B and C went after it, so mpv plays C after B.
        // (The player corrects B long before; without one, mpv's choice
        // is reported as it is.)
        ev(&mut s, FILE_LOADED);
        let out = line(&mut s, &end_file(2, "eof"), Instant::now());
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(2) }]);
        switch_check(&out);
        assert_eq!(
            ev(&mut s, &start_file(3)),
            [PlaybackEvent::Advanced {
                from: l(2),
                to: l(3)
            }]
        );
    }

    #[test]
    fn a_start_of_an_unknown_entry_ends_the_switch() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        line(&mut s, &end_file(1, "eof"), Instant::now());
        let out = line(&mut s, &start_file(9), Instant::now());
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
        // And it is stopped, as any entry Coda never requested.
        assert_eq!(commands(&out), [MpvCommand::Stop]);
    }

    #[test]
    fn a_stale_end_never_waits() {
        // A newer load is on its way: the current entry's end is stale.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        load(&mut s, 5, "/m/x.flac");
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.requests.is_empty());
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
        // So does a stop.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        s.handle_command(PlaybackCommand::Stop);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.requests.is_empty());
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn a_stop_or_load_during_the_switch_makes_its_outcome_stale() {
        for stop in [true, false] {
            let mut s = MpvState::new();
            playing_with_next(&mut s);
            line(&mut s, &end_file(1, "eof"), Instant::now());
            if stop {
                s.handle_command(PlaybackCommand::Stop);
            } else {
                load(&mut s, 5, "/m/x.flac");
            }
            // mpv had gone on to B: A simply ended, and B is no advance.
            assert_eq!(
                ev(&mut s, &start_file(2)),
                [PlaybackEvent::TrackEnded {
                    load: l(1),
                    reason: EndReason::Eof
                }],
                "stop: {stop}"
            );
            assert_eq!(
                ev(&mut s, FILE_LOADED),
                [PlaybackEvent::Started { load: l(2) }]
            );
        }
    }

    #[test]
    fn started_waits_until_the_next_track_is_heard() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let pos = |s: &mut MpvState, secs: f64, ms: u64| line(s, &time_pos(secs), at(ms)).events;
        // mpv 0.33/0.35: negative positions during the overlap.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        line(&mut s, &end_file(1, "eof"), at(0));
        line(&mut s, &start_file(2), at(0));
        line(&mut s, FILE_LOADED, at(20));
        for (secs, ms) in [(-0.33, 300), (-0.2, 600), (0.0, 900)] {
            assert!(
                !pos(&mut s, secs, ms)
                    .iter()
                    .any(|e| matches!(e, PlaybackEvent::Started { .. })),
                "{secs}"
            );
        }
        let events = pos(&mut s, 0.02, 1200);
        assert_eq!(events[0], PlaybackEvent::Started { load: l(2) });
        // Once only.
        assert!(
            !pos(&mut s, 0.3, 1500)
                .iter()
                .any(|e| matches!(e, PlaybackEvent::Started { .. }))
        );

        // mpv 0.41: 0 at `file-loaded`, then nothing until it is heard.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        line(&mut s, &end_file(1, "eof"), at(0));
        line(&mut s, &start_file(2), at(0));
        pos(&mut s, 0.0, 10);
        line(&mut s, FILE_LOADED, at(20));
        assert!(
            !pos(&mut s, 0.0, 300)
                .iter()
                .any(|e| matches!(e, PlaybackEvent::Started { .. }))
        );
        let events = pos(&mut s, 0.112, 600);
        assert_eq!(events[0], PlaybackEvent::Started { load: l(2) });

        // A positive position before `file-loaded` cannot be the new
        // track's (mpv has none yet): it is ignored for this.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        line(&mut s, &end_file(1, "eof"), at(0));
        line(&mut s, &start_file(2), at(0));
        assert!(
            !pos(&mut s, 2.9, 10)
                .iter()
                .any(|e| matches!(e, PlaybackEvent::Started { .. }))
        );
    }

    #[test]
    fn a_very_short_next_track_starts_before_it_ends() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        ev(&mut s, &end_file(1, "eof"));
        ev(&mut s, &start_file(2));
        ev(&mut s, FILE_LOADED);
        assert_eq!(
            ev(&mut s, &end_file(2, "eof")),
            [
                PlaybackEvent::Started { load: l(2) },
                PlaybackEvent::TrackEnded {
                    load: l(2),
                    reason: EndReason::Eof
                }
            ]
        );
    }

    #[test]
    fn a_next_track_that_fails_ends_with_an_error_after_the_advance() {
        // A 404 stream or a file deleted since: mpv reports it only at the
        // change, then stops.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        ev(&mut s, &end_file(1, "eof"));
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
        let ended = ev(&mut s, &end_file(2, "error"));
        assert_eq!(
            ended,
            [PlaybackEvent::TrackEnded {
                load: l(2),
                reason: EndReason::Error("playback error".into())
            }],
            "never Started: it was not opened"
        );
    }

    #[test]
    fn the_track_after_a_gapless_change_can_go_on_without_a_gap_too() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        ev(&mut s, &end_file(1, "eof"));
        ev(&mut s, &start_file(2));
        ev(&mut s, FILE_LOADED);
        // The player announces what follows B.
        let out = set_next(&mut s, 2, Some((3, "/m/c.flac")));
        assert_eq!(
            commands(&out),
            [MpvCommand::PlaylistClear, append("/m/c.flac")]
        );
        ack_next(&mut s, &out, 3);
        // A next entry for A is stale now.
        assert_eq!(
            set_next(&mut s, 1, Some((9, "/m/z.flac"))),
            Output::default()
        );
        let out = line(&mut s, &end_file(2, "eof"), Instant::now());
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(2) }]);
        assert_eq!(
            ev(&mut s, &start_file(3)),
            [PlaybackEvent::Advanced {
                from: l(2),
                to: l(3)
            }]
        );
    }

    #[test]
    fn a_rejected_load_holds_no_next_entry() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        s.load_missing(l(2));
        assert_eq!(
            set_next(&mut s, 2, Some((3, "/m/c.flac"))),
            Output::default()
        );
    }

    #[test]
    fn a_refused_or_unmapped_append_is_not_a_successor() {
        // Refused: not held, and the end is an ordinary EOF.
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        ev(&mut s, &start_file(1));
        let out = set_next(&mut s, 1, Some((2, "/m/b.flac")));
        let append_q = request_id(&out, |c| matches!(c, MpvCommand::AppendFile { .. }));
        reply_ok(&mut s, out.requests[0].request_id, Value::Null);
        reply_err(&mut s, append_q, "invalid parameter");
        assert!(s.lookahead_enabled());
        assert_eq!(
            ev(&mut s, &end_file(1, "eof")),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );

        // No entry id in the reply: the lookahead is off for the session
        // and what was appended is removed again.
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        let out = set_next(&mut s, 1, Some((2, "/m/b.flac")));
        let append_q = request_id(&out, |c| matches!(c, MpvCommand::AppendFile { .. }));
        reply_ok(&mut s, out.requests[0].request_id, Value::Null);
        let out = reply_ok(&mut s, append_q, json!({}));
        assert_eq!(commands(&out), [MpvCommand::PlaylistClear]);
        assert!(!s.lookahead_enabled());
        assert_eq!(
            set_next(&mut s, 1, Some((3, "/m/c.flac"))),
            Output::default()
        );
    }

    #[test]
    fn appended_entries_that_can_no_longer_play_are_forgotten() {
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        // Superseded by C (entry 3); then a load replaces everything.
        let out = set_next(&mut s, 1, Some((3, "/m/c.flac")));
        ack_next(&mut s, &out, 3);
        let out = load(&mut s, 4, "/m/d.flac");
        map(&mut s, loadfile_id(&out), 4);
        ev(&mut s, &end_file(1, "stop"));
        ev(&mut s, &start_file(4));
        assert_eq!(s.entries.len(), 1, "{:?}", s.entries);
        assert!(s.entries.contains_key(&4));
    }

    #[test]
    fn a_gapless_change_hands_the_visualizer_over_and_keeps_its_graph() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        line(&mut s, r#"{"event":"playback-restart"}"#, at(0));
        let out = set_next(&mut s, 1, Some((2, "/m/b.flac")));
        ack_next(&mut s, &out, 2);
        line(&mut s, &time_pos(2.7), at(0));
        let reading = |t: f64, db: &str| {
            let mut map = serde_json::Map::new();
            map.insert("lavfi.astats.1.RMS_level".into(), json!(db));
            map.insert("lavfi.astats.2.RMS_level".into(), json!(db));
            let tc = format!("{:.6}", 20.0 * (t / 32.0).log10());
            map.insert("lavfi.astats.10.Peak_level".into(), json!(tc));
            Value::Object(map)
        };
        let mut all = Vec::new();
        let poll =
            |s: &mut MpvState, all: &mut Vec<MpvCommand>, ms: u64, answer: Option<(f64, &str)>| {
                let out = s.visualizer_tick(at(ms));
                all.extend(af_requests(&out));
                if let (Some(r), Some((t, db))) = (out.requests.first(), answer) {
                    let reply = s.handle_message(
                        MpvMessage::Reply {
                            request_id: r.request_id,
                            result: Ok(Some(reading(t, db))),
                        },
                        at(ms + 1),
                    );
                    all.extend(af_requests(&reply));
                }
                out.events
            };
        // A's last readings, 0.25 s ahead of what is heard.
        poll(&mut s, &mut all, 0, Some((2.95, "-6")));
        // A's decode ends; mpv goes on to B.
        let out = line(&mut s, &end_file(1, "eof"), at(40));
        all.extend(af_requests(&out));
        let out = line(&mut s, &start_file(2), at(40));
        all.extend(af_requests(&out));
        assert_eq!(
            out.events,
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
        line(&mut s, FILE_LOADED, at(50));
        // mpv reports B's position now (not heard yet); A's clock goes on.
        line(&mut s, &time_pos(-0.3), at(60));
        // B's first readings (time code from 0) while A is still heard.
        assert!(poll(&mut s, &mut all, 80, Some((0.05, "-60"))).is_empty());
        assert!(poll(&mut s, &mut all, 120, Some((0.1, "-60"))).is_empty());
        // A's reading is shown when A's clock reaches it, as A's.
        let events = poll(&mut s, &mut all, 260, None);
        let [PlaybackEvent::Visualizer { load, frame }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!(*load, l(1));
        assert!(frame.levels[0] > 0.5, "{frame:?}");
        // B is heard: `Started`, and B's readings take over.
        let out = line(&mut s, &time_pos(0.11), at(300));
        assert_eq!(out.events[0], PlaybackEvent::Started { load: l(2) });
        let events = poll(&mut s, &mut all, 340, None);
        let [PlaybackEvent::Visualizer { load, frame }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!(*load, l(2));
        assert_eq!(frame.levels[0], 0.0);
        assert!(all.is_empty(), "the graph is never touched: {all:?}");
    }

    #[test]
    fn a_hidden_graph_stays_through_a_gapless_change() {
        let mut s = MpvState::new();
        s.handle_command(PlaybackCommand::SetVisualizer(true));
        let out = started(&mut s);
        reply_ok(&mut s, request_id(&out, ADD), Value::Null);
        s.handle_command(PlaybackCommand::SetVisualizer(false));
        let out = set_next(&mut s, 1, Some((2, "/m/b.flac")));
        ack_next(&mut s, &out, 2);
        for msg in [
            end_file(1, "eof"),
            start_file(2),
            FILE_LOADED.to_owned(),
            time_pos(0.2),
        ] {
            let out = line(&mut s, &msg, Instant::now());
            assert!(af_requests(&out).is_empty(), "{msg}: {out:?}");
        }
        // Removed at the next load, as before.
        let out = load(&mut s, 3, "/m/c.flac");
        assert_eq!(af_requests(&out), [REMOVE]);
    }

    #[test]
    fn a_stream_is_appended_with_its_url() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        let url = url::Url::parse("http://127.0.0.1:1/rest/stream?id=x&t=tok").unwrap();
        let stream = crate::playback::StreamUrl::new(url).unwrap();
        let out = s.handle_command(PlaybackCommand::SetNext {
            after: l(1),
            next: Some((l(2), PlaybackTarget::Stream(stream.clone()))),
        });
        assert_eq!(
            commands(&out),
            [
                MpvCommand::PlaylistClear,
                MpvCommand::AppendStream { url: stream }
            ]
        );
        assert!(out.requests[1].command.is_sensitive());
    }

    #[test]
    fn many_next_changes_within_one_track_keep_the_current_entry() {
        // Holding `D` on the entry after the current one, or `s` again and
        // again: every press is a new next entry.
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        ev(&mut s, &start_file(1));
        ev(&mut s, FILE_LOADED);
        let mut entry = 1;
        for k in 0..(2 * MAX_ENTRY_MAPPINGS as u64) {
            let out = set_next(&mut s, 1, Some((100 + k, "/m/n.flac")));
            entry += 1;
            ack_next(&mut s, &out, entry);
            assert!(s.entries.len() <= 3, "{k}: {:?}", s.entries);
            assert!(s.entries.contains_key(&1), "{k}: the current entry");
        }
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.events.is_empty(), "{out:?}");
        switch_check(&out);
        assert_eq!(
            ev(&mut s, &start_file(entry)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(100 + 2 * MAX_ENTRY_MAPPINGS as u64 - 1)
            }]
        );
    }

    #[test]
    fn the_entry_before_the_newest_append_stays_recognised() {
        // mpv may have moved to B just before the clear for C ran; B's
        // `start-file` then arrives after C's append reply.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = set_next(&mut s, 1, Some((3, "/m/c.flac")));
        ack_next(&mut s, &out, 3);
        ev(&mut s, &end_file(1, "eof"));
        assert_eq!(
            ev(&mut s, &start_file(2)),
            [PlaybackEvent::Advanced {
                from: l(1),
                to: l(2)
            }]
        );
    }

    #[test]
    fn a_current_entry_not_started_yet_survives_next_changes() {
        let mut s = MpvState::new();
        let out = load(&mut s, 1, "/m/a.flac");
        map(&mut s, loadfile_id(&out), 1);
        for (k, entry) in [(2, 2), (3, 3), (4, 4)] {
            let out = set_next(&mut s, 1, Some((k, "/m/b.flac")));
            ack_next(&mut s, &out, entry);
        }
        ev(&mut s, &start_file(1));
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(1) }]
        );
    }

    #[test]
    fn a_next_entry_held_during_a_switch_is_sent_once_it_ends() {
        // A ends with B held; the user plays C (with D next) at that moment.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        let check = switch_check(&out);
        let out = load(&mut s, 3, "/m/c.flac");
        let c = loadfile_id(&out);
        assert_eq!(
            set_next(&mut s, 3, Some((4, "/m/d.flac"))),
            Output::default()
        );
        reply_ok(&mut s, check, json!(false));
        let out = map(&mut s, c, 3);
        assert!(commands(&out).is_empty(), "still switching: {out:?}");
        // mpv had gone on to B; then C replaced it.
        let out = line(&mut s, &start_file(2), Instant::now());
        assert_eq!(
            commands(&out),
            [MpvCommand::PlaylistClear, append("/m/d.flac")]
        );
        // The same when mpv says it stopped.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        let check = switch_check(&out);
        let out = load(&mut s, 3, "/m/c.flac");
        map(&mut s, loadfile_id(&out), 3);
        set_next(&mut s, 3, Some((4, "/m/d.flac")));
        let out = reply_ok(&mut s, check, json!(true));
        assert_eq!(
            commands(&out),
            [MpvCommand::PlaylistClear, append("/m/d.flac")]
        );
    }

    #[test]
    fn mpvs_idle_event_ends_a_switch() {
        // Should a hook ever run between `end-file` and `start-file`, the
        // check could be answered first; `idle` then says nothing follows.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        ev(&mut s, &end_file(1, "eof"));
        assert_eq!(
            ev(&mut s, r#"{"event":"idle"}"#),
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
        // Outside a switch, `idle` means nothing.
        assert!(ev(&mut s, r#"{"event":"idle"}"#).is_empty());
    }

    #[test]
    fn a_load_the_driver_rejected_leaves_nothing_current() {
        // A plays with B held; the user's load of a missing file is
        // rejected. A's end is then no advance for anyone.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        s.load_missing(l(5));
        let out = line(&mut s, &end_file(1, "eof"), Instant::now());
        assert!(out.requests.is_empty());
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Eof
            }]
        );
    }

    #[test]
    fn an_append_behind_the_advanced_entry_is_cleared_unless_replaced() {
        // B (entry 2) held; a change to C (entry 3) ran after mpv moved on.
        for player_next in [None, Some((9, "/m/d.flac"))] {
            let mut s = MpvState::new();
            playing_with_next(&mut s);
            let out = set_next(&mut s, 1, Some((3, "/m/c.flac")));
            ack_next(&mut s, &out, 3);
            ev(&mut s, &end_file(1, "eof"));
            let out = line(&mut s, &start_file(2), Instant::now());
            assert_eq!(
                commands(&out),
                [MpvCommand::PlaylistClear],
                "C must not follow B"
            );
            ack_next(&mut s, &out, 0);
            // The player's own announcement for B is not lost either way.
            let out = set_next(&mut s, 2, player_next);
            match player_next {
                None => assert_eq!(out, Output::default()),
                Some(_) => assert_eq!(
                    commands(&out),
                    [MpvCommand::PlaylistClear, append("/m/d.flac")]
                ),
            }
        }
        // While replies are still due, the clear waits for them, and the
        // player's "nothing next" does not cancel it.
        let mut s = MpvState::new();
        playing_with_next(&mut s);
        let pending = set_next(&mut s, 1, Some((3, "/m/c.flac")));
        ev(&mut s, &end_file(1, "eof"));
        let out = line(&mut s, &start_file(2), Instant::now());
        assert!(commands(&out).is_empty());
        assert_eq!(set_next(&mut s, 2, None), Output::default());
        let out = ack_next(&mut s, &pending, 3);
        assert_eq!(commands(&out), [MpvCommand::PlaylistClear]);
    }
}
