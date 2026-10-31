//! Pure mpv backend state: turns [`PlaybackCommand`]s and parsed mpv
//! messages into mpv requests and [`PlaybackEvent`]s. No I/O and no Tokio;
//! the async driver feeds it and performs the writes.
//!
//! Stale-event core (brief section 3.5):
//! 1. Loads are serialized. While one is in flight, further loads collapse
//!    into a single queued load (the latest wins).
//! 2. A load sends `set_property pause false`, `loadfile <path> replace` and
//!    `get_property playlist/0/id`. Coda is the only playlist writer and
//!    `replace` leaves exactly one entry, so the reply is the new entry id,
//!    which is mapped to the load's [`LoadId`].
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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::protocol::{
    self, EndFileReason, MpvCommand, MpvEvent, MpvMessage, ProtocolError, SeekMode,
};
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackTarget};

/// Minimum spacing between emitted `Position` events (at most 4/s).
pub const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// Bound on file events buffered while an entry-id mapping is pending.
pub const MAX_BUFFERED_EVENTS: usize = 16;

/// Bound on remembered entry-id mappings. Mappings are removed on
/// `end-file`; this only guards against unbounded growth.
const MAX_ENTRY_MAPPINGS: usize = 64;

pub const ENTRY_ID_PROPERTY: &str = "playlist/0/id";
pub const OBSERVED_PROPERTIES: [&str; 4] = ["time-pos", "duration", "pause", "volume"];
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
    LoadFile(LoadId),
    EntryId(LoadId),
}

#[derive(Debug)]
struct InFlight {
    load: LoadId,
    /// The `loadfile` reply was an error; no entry will be mapped.
    failed: bool,
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
    entries: HashMap<i64, LoadId>,
    /// Highest entry id ever mapped. mpv entry ids only increase, so an
    /// unmapped id at or below this can never become ours.
    max_mapped: Option<i64>,
    /// Entry of the latest `start-file` that has not ended yet; receives
    /// the next `file-loaded`.
    opening: Option<i64>,
    buffered: VecDeque<FileEvent>,
    position: PositionCoalescer,
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

    pub fn handle_command(&mut self, command: PlaybackCommand) -> Output {
        let mut out = Output::default();
        match command {
            PlaybackCommand::Load { load, target } => {
                if self.in_flight.is_some() {
                    // Collapse: only the latest queued load survives.
                    if let Some((dropped, _)) = self.queued.replace((load, target)) {
                        tracing::debug!(%load, %dropped, "load superseded before reaching mpv");
                    }
                } else {
                    self.start_load(&mut out, load, target);
                }
            }
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
                self.request(&mut out, MpvCommand::Stop, Pending::Ignore);
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
            MpvMessage::Reply { request_id, result } => self.on_reply(&mut out, request_id, result),
            MpvMessage::Event(event) => self.on_event(&mut out, event, now),
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
            self.request(out, MpvCommand::Seek { seconds, mode }, Pending::Ignore);
        }
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
        self.in_flight = Some(InFlight {
            load,
            failed: false,
        });
        self.request(
            out,
            MpvCommand::SetProperty {
                name: "pause",
                value: false.into(),
            },
            Pending::Ignore,
        );
        self.request(out, command, Pending::LoadFile(load));
        self.request(
            out,
            MpvCommand::GetProperty {
                name: ENTRY_ID_PROPERTY,
            },
            Pending::EntryId(load),
        );
    }

    fn reject_load(&mut self, out: &mut Output, load: LoadId, message: String) {
        if let Some((dropped, _)) = self.queued.take() {
            tracing::debug!(%dropped, %load, "queued load superseded by rejected load");
        }
        tracing::debug!(%load, reason = %message, "load rejected before reaching mpv");
        self.position.clear();
        out.events.push(PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Error(message),
        });
    }

    fn on_reply(
        &mut self,
        out: &mut Output,
        request_id: u64,
        result: Result<Option<Value>, String>,
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
            Pending::LoadFile(load) => {
                if let Err(error) = result {
                    tracing::warn!(%load, %error, "mpv rejected loadfile");
                    if let Some(in_flight) = self.in_flight.as_mut().filter(|f| f.load == load) {
                        in_flight.failed = true;
                    }
                    self.position.clear();
                    out.events.push(PlaybackEvent::TrackEnded {
                        load,
                        reason: EndReason::Error(format!("mpv could not load the file: {error}")),
                    });
                }
            }
            Pending::EntryId(load) => self.on_entry_id(out, load, result),
        }
    }

    fn on_entry_id(
        &mut self,
        out: &mut Output,
        load: LoadId,
        result: Result<Option<Value>, String>,
    ) {
        let in_flight = match self.in_flight.take() {
            Some(f) if f.load == load => f,
            other => {
                // Unreachable by construction (one load in flight); be safe.
                self.in_flight = other;
                tracing::warn!(%load, "entry id reply for a load that is not in flight");
                return;
            }
        };
        let buffered = std::mem::take(&mut self.buffered);
        if !in_flight.failed {
            match result.ok().flatten().and_then(|v| v.as_i64()) {
                Some(entry_id) => {
                    tracing::debug!(%load, entry_id, "mapped mpv playlist entry");
                    self.map_entry(entry_id, load);
                    for event in buffered {
                        if event.entry_id == entry_id {
                            self.deliver(out, event, load);
                        } else {
                            tracing::debug!(?event, "dropping buffered event for unmapped entry");
                        }
                    }
                }
                None => {
                    tracing::warn!(%load, "mpv did not report the new playlist entry");
                    out.events.push(PlaybackEvent::TrackEnded {
                        load,
                        reason: EndReason::Error("mpv did not report the playlist entry".into()),
                    });
                }
            }
        }
        if let Some((next, target)) = self.queued.take() {
            self.start_load(out, next, target);
        }
    }

    fn map_entry(&mut self, entry_id: i64, load: LoadId) {
        if self.entries.len() >= MAX_ENTRY_MAPPINGS
            && let Some(&oldest) = self.entries.keys().min()
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(entry_id, load);
        self.max_mapped = Some(self.max_mapped.map_or(entry_id, |m| m.max(entry_id)));
    }

    fn on_event(&mut self, out: &mut Output, event: MpvEvent, now: Instant) {
        let file_event = match event {
            MpvEvent::StartFile {
                entry_id: Some(entry_id),
            } => {
                self.opening = Some(entry_id);
                FileEvent {
                    entry_id,
                    kind: FileEventKind::Start,
                }
            }
            MpvEvent::FileLoaded => match self.opening {
                Some(entry_id) => FileEvent {
                    entry_id,
                    kind: FileEventKind::Loaded,
                },
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
            MpvEvent::Other { .. } => return,
        };
        self.route(out, file_event);
    }

    fn route(&mut self, out: &mut Output, event: FileEvent) {
        if let Some(&load) = self.entries.get(&event.entry_id) {
            self.deliver(out, event, load);
            return;
        }
        let mapping_pending = self.in_flight.as_ref().is_some_and(|f| !f.failed);
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
            FileEventKind::Start => tracing::debug!(%load, "mpv opening file"),
            FileEventKind::Loaded => out.events.push(PlaybackEvent::Started { load }),
            FileEventKind::End { reason, file_error } => {
                self.entries.remove(&event.entry_id);
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
            }
        }
    }

    fn on_property(&mut self, out: &mut Output, name: &str, data: Option<Value>, now: Instant) {
        match name {
            "time-pos" => {
                if let Some(position) = data.and_then(|v| seconds(&v))
                    && let Some(position) = self.position.offer(position, now)
                {
                    out.events.push(PlaybackEvent::Position(position));
                }
            }
            "duration" => out
                .events
                .push(PlaybackEvent::Duration(data.and_then(|v| seconds(&v)))),
            "pause" => {
                if let Some(paused) = data.and_then(|v| v.as_bool()) {
                    out.events.push(PlaybackEvent::Paused(paused));
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

    /// Request ids of (loadfile, get_property) in a load's output.
    fn load_ids(out: &Output) -> (u64, u64) {
        let find = |pred: fn(&MpvCommand) -> bool| {
            out.requests
                .iter()
                .find(|r| pred(&r.command))
                .map(|r| r.request_id)
                .expect("request present")
        };
        (
            find(|c| {
                matches!(
                    c,
                    MpvCommand::LoadFile { .. } | MpvCommand::LoadStream { .. }
                )
            }),
            find(|c| matches!(c, MpvCommand::GetProperty { .. })),
        )
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
    fn load_sends_unpause_loadfile_then_entry_id_query() {
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
                MpvCommand::GetProperty {
                    name: ENTRY_ID_PROPERTY
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
        let (_, entry_q) = load_ids(&out);
        // Replies out of order: the entry-id reply is still matched to load 1.
        assert!(reply_ok(&mut s, entry_q, json!(5)).events.is_empty());
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
        let (file_q, entry_q) = load_ids(&out);
        reply_ok(&mut s, file_q, json!({"playlist_entry_id": 7}));
        reply_ok(&mut s, entry_q, json!(7));
        assert!(!s.load_in_flight());
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
            let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
            reply_ok(&mut s, entry_q, json!(1));
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
    fn events_before_entry_id_reply_are_buffered_and_replayed() {
        let mut s = MpvState::new();
        let (file_q, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        assert!(ev(&mut s, &start_file(3)).is_empty());
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        assert!(ev(&mut s, &end_file(3, "eof")).is_empty());
        reply_ok(&mut s, file_q, Value::Null);
        let out = reply_ok(&mut s, entry_q, json!(3));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        assert!(ev(&mut s, &end_file(41, "eof")).is_empty());
        let out = reply_ok(&mut s, entry_q, json!(43));
        assert!(out.events.is_empty());
        // Events without an entry id are ignored.
        assert!(ev(&mut s, r#"{"event":"end-file","reason":"eof"}"#).is_empty());
    }

    #[test]
    fn buffer_is_bounded_and_drops_newest() {
        let mut s = MpvState::new();
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        // The real events arrive first and must survive a later flood.
        assert!(ev(&mut s, &start_file(2)).is_empty());
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        for i in 0..2 * MAX_BUFFERED_EVENTS {
            ev(&mut s, &end_file(100 + i as i64, "stop"));
        }
        assert_eq!(s.buffered.len(), MAX_BUFFERED_EVENTS);
        let out = reply_ok(&mut s, entry_q, json!(2));
        assert_eq!(out.events, [PlaybackEvent::Started { load: l(1) }]);
        assert!(s.buffered.is_empty());
    }

    #[test]
    fn ids_not_newer_than_the_highest_mapped_id_are_never_buffered() {
        let mut s = MpvState::new();
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        reply_ok(&mut s, entry_q, json!(5));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        reply_ok(&mut s, entry_q, json!(1));
        // No start-file yet: nothing to attribute.
        assert!(ev(&mut s, FILE_LOADED).is_empty());
        ev(&mut s, &start_file(1));
        assert_eq!(
            ev(&mut s, FILE_LOADED),
            [PlaybackEvent::Started { load: l(1) }]
        );
        // A file that fails to open: start-file, end-file{error}, no Started.
        let (_, entry_q) = load_ids(&load(&mut s, 2, "/m/b.flac"));
        reply_ok(&mut s, entry_q, json!(2));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/evil.mp3"));
        reply_ok(&mut s, entry_q, json!(1));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/evil.mp3"));
        reply_ok(&mut s, entry_q, json!(1));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        reply_ok(&mut s, entry_q, json!(3));
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
        let (_, entry_q) = load_ids(&first);
        for (n, p) in [(2, "/m/b.flac"), (3, "/m/c.flac"), (4, "/m/d.flac")] {
            assert_eq!(load(&mut s, n, p), Output::default());
        }
        let out = reply_ok(&mut s, entry_q, json!(1));
        assert_eq!(loadfile_paths(&out), ["/m/d.flac"]);
        assert!(s.load_in_flight());
        let (_, entry_q) = load_ids(&out);
        reply_ok(&mut s, entry_q, json!(2));
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
        let (file_q, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        load(&mut s, 2, "/m/b.flac");
        let out = reply_err(&mut s, file_q, "invalid parameter");
        assert_eq!(
            out.events,
            [PlaybackEvent::TrackEnded {
                load: l(1),
                reason: EndReason::Error("mpv could not load the file: invalid parameter".into())
            }]
        );
        // The entry-id reply (which would name the *previous* entry) is not
        // mapped, and the queued load proceeds.
        let out = reply_ok(&mut s, entry_q, json!(9));
        assert!(out.events.is_empty());
        assert_eq!(loadfile_paths(&out), ["/m/b.flac"]);
        assert!(ev(&mut s, &end_file(9, "eof")).is_empty());
    }

    #[test]
    fn failed_entry_id_query_ends_the_load_with_error() {
        let mut s = MpvState::new();
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
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
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
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
        assert!(loadfile_paths(&reply_ok(&mut s, entry_q, json!(1))).is_empty());
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
        assert!(load_ids(&out).1 > 0, "entry id is queried as for files");
    }

    #[test]
    fn stop_cancels_queued_load() {
        let mut s = MpvState::new();
        let (_, entry_q) = load_ids(&load(&mut s, 1, "/m/a.flac"));
        load(&mut s, 2, "/m/b.flac");
        let out = s.handle_command(PlaybackCommand::Stop);
        assert_eq!(out.requests.len(), 1);
        assert_eq!(out.requests[0].command, MpvCommand::Stop);
        assert!(loadfile_paths(&reply_ok(&mut s, entry_q, json!(1))).is_empty());
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
        let (file_q, entry_q) = load_ids(&first);
        reply_ok(&mut s, first.requests[0].request_id, Value::Null);
        reply_ok(&mut s, file_q, Value::Null);
        reply_ok(&mut s, entry_q, json!(1));
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
}
