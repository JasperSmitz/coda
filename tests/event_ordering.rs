//! Event-ordering safety (owner requirement, M1 brief section 4; M8).
//!
//! Drives the pure mpv state machine and the pure player together through
//! scripted mpv message sequences and proves that entry ids and
//! asynchronous events never advance the queue incorrectly. Every scenario
//! asserts the final queue position and the exact number of `Load`
//! commands the player emitted.
//!
//! The simulated mpv holds Coda's current entry and, with the gapless
//! lookahead (M8), a copy of the next one. Scripts decide what mpv does
//! with it: which entry it starts after the current one ends, and in which
//! order its replies and events reach Coda. mpv runs IPC commands
//! synchronously and writes the events that happened meanwhile after the
//! reply, so a reply can come before an event that happened before its
//! command ran; the scripts allow every order that is possible.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use coda_player::model::{Track, TrackMetadata};
use coda_player::playback::mpv::protocol::MpvCommand;
use coda_player::playback::mpv::state::{
    IDLE_PROPERTY, MpvState, OutgoingRequest, Output, POSITION_INTERVAL,
};
use coda_player::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackTarget};
use coda_player::player::{NO_PLAYABLE_TRACKS, PlaybackStatus, Player, PlayerCommand, RepeatMode};
use coda_player::queue::QueueEntryId;
use coda_player::resolve::resolve_command;
use serde_json::json;

struct Sim {
    player: Player,
    mpv: MpvState,
    ids: Vec<QueueEntryId>,
    /// Every request written to mpv, in order.
    sent: Vec<OutgoingRequest>,
    /// `Load` commands emitted by the player.
    loads: Vec<LoadId>,
    /// `Started` events (mpv `file-loaded`) for the player's current load.
    started_current: Vec<LoadId>,
    /// `TrackEnded { Eof | Error }` for the current load (real advances).
    advances: usize,
    /// `Advanced` events the player followed (gapless advances).
    gapless: usize,
    /// `Advanced` events from the current load to an entry the player no
    /// longer wanted next, which it corrected.
    corrections: usize,
    /// `Started` events for loads that were not current (stale).
    started_stale: usize,
    messages: Vec<String>,
    /// Paths whose files are gone: the driver's precheck rejects their
    /// loads and does not append them.
    missing: Vec<String>,
    /// Requests mpv has answered.
    answered: Vec<u64>,
    /// Simulated time: each mpv message arrives one position interval
    /// after the previous one, so position updates are never coalesced
    /// away.
    clock: Instant,
}

impl Sim {
    fn new(names: &[&str]) -> Self {
        let mut player = Player::new();
        let ids = names
            .iter()
            .map(|n| player.enqueue(track(n)).unwrap())
            .collect();
        let mut mpv = MpvState::new();
        let startup = mpv.startup();
        let mut sim = Self {
            player,
            mpv,
            ids,
            sent: Vec::new(),
            loads: Vec::new(),
            started_current: Vec::new(),
            advances: 0,
            gapless: 0,
            corrections: 0,
            started_stale: 0,
            messages: Vec::new(),
            missing: Vec::new(),
            answered: Vec::new(),
            clock: Instant::now(),
        };
        sim.process(startup);
        sim
    }

    /// Resolves player commands as the app does (local tracks only, no
    /// Navidrome client), applies them to mpv and feeds resulting events
    /// back, until quiescent. Like the app's dispatch, it lets the player
    /// announce its next entry once a batch is done.
    fn commands(&mut self, commands: Vec<PlayerCommand>) {
        // Messages produced by the intent that created `commands`.
        self.messages.extend(self.player.take_messages());
        let mut work: VecDeque<PlayerCommand> = commands.into();
        while let Some(command) = work.pop_front().or_else(|| self.player.sync_next()) {
            let command = match resolve_command(command, None) {
                Ok(command) => command,
                Err((load, reason)) => panic!("{load}: local load not resolved: {reason}"),
            };
            let out = match command {
                PlaybackCommand::Load {
                    load,
                    target: PlaybackTarget::File(path),
                } => {
                    self.loads.push(load);
                    if self.is_missing(&path) {
                        self.mpv.load_rejected(load, "file not found")
                    } else {
                        self.mpv.handle_command(PlaybackCommand::Load {
                            load,
                            target: PlaybackTarget::File(path),
                        })
                    }
                }
                PlaybackCommand::SetNext {
                    after,
                    next: Some((load, PlaybackTarget::File(path))),
                } => {
                    let next =
                        (!self.is_missing(&path)).then_some((load, PlaybackTarget::File(path)));
                    self.mpv
                        .handle_command(PlaybackCommand::SetNext { after, next })
                }
                command => self.mpv.handle_command(command),
            };
            work.extend(self.process(out));
        }
    }

    fn is_missing(&self, path: &std::path::Path) -> bool {
        self.missing.iter().any(|m| path.to_str() == Some(m))
    }

    /// Records requests and delivers events to the player; returns the
    /// player's follow-up commands.
    fn process(&mut self, out: Output) -> Vec<PlayerCommand> {
        self.sent.extend(out.requests);
        let mut follow_up = Vec::new();
        for event in out.events {
            let current = self.player.current_load();
            match &event {
                PlaybackEvent::Started { load } => {
                    if current == Some(*load) {
                        self.started_current.push(*load);
                    } else {
                        self.started_stale += 1;
                    }
                }
                PlaybackEvent::TrackEnded {
                    load,
                    reason: EndReason::Eof | EndReason::Error(_),
                } if current == Some(*load) => self.advances += 1,
                _ => {}
            }
            let advanced = match &event {
                PlaybackEvent::Advanced { from, to } if current == Some(*from) => Some(*to),
                _ => None,
            };
            follow_up.extend(self.player.handle_event(event));
            if let Some(to) = advanced {
                if self.player.current_load() == Some(to) {
                    self.gapless += 1;
                } else {
                    self.corrections += 1;
                }
            }
        }
        self.messages.extend(self.player.take_messages());
        follow_up
    }

    fn mpv_message(&mut self, line: &str) {
        self.clock += POSITION_INTERVAL;
        let out = self.mpv.handle_line(line, self.clock).expect("valid line");
        let follow_up = self.process(out);
        self.commands(follow_up);
    }

    // ----- user intents --------------------------------------------------

    fn play(&mut self, index: usize) {
        let commands = self.player.play_entry(self.ids[index]);
        self.commands(commands);
    }

    fn press_n(&mut self) {
        let commands = self.player.play_next();
        self.commands(commands);
    }

    /// Removes the entry after the current one.
    fn remove_next(&mut self) {
        let current = self.player.queue().current().expect("a current entry");
        let next = self.player.queue().id_after(current).expect("a next entry");
        let commands = self.player.remove(next);
        self.commands(commands);
    }

    /// `A` on a new track.
    fn play_next(&mut self, name: &str) {
        self.player.enqueue_next(track(name)).unwrap();
        self.commands(Vec::new());
    }

    fn clear_queue(&mut self) {
        let commands = self.player.clear_queue();
        self.commands(commands);
    }

    // ----- scripted mpv messages ----------------------------------------

    fn requests(&self, pred: impl Fn(&MpvCommand) -> bool) -> Vec<&OutgoingRequest> {
        self.sent.iter().filter(|r| pred(&r.command)).collect()
    }

    /// Paths of `loadfile … replace`, in order.
    fn sent_paths(&self) -> Vec<String> {
        self.requests(|c| matches!(c, MpvCommand::LoadFile { .. }))
            .into_iter()
            .map(|r| match &r.command {
                MpvCommand::LoadFile { path } => path.clone(),
                _ => unreachable!(),
            })
            .collect()
    }

    /// Paths of `loadfile … append`, in order.
    fn appended_paths(&self) -> Vec<String> {
        self.requests(|c| matches!(c, MpvCommand::AppendFile { .. }))
            .into_iter()
            .map(|r| match &r.command {
                MpvCommand::AppendFile { path } => path.clone(),
                _ => unreachable!(),
            })
            .collect()
    }

    /// The request id of the `n`th request matching `pred`, if sent.
    fn nth(&self, n: usize, pred: impl Fn(&MpvCommand) -> bool) -> Option<u64> {
        self.requests(pred).get(n).map(|r| r.request_id)
    }

    fn reply(&mut self, request_id: u64, data: serde_json::Value) {
        self.answered.push(request_id);
        self.mpv_message(
            &json!({"data": data, "request_id": request_id, "error": "success"}).to_string(),
        );
    }

    /// mpv's reply to the `n`th `loadfile … replace`, naming `entry`.
    fn loadfile_ok(&mut self, n: usize, entry: i64) {
        let id = self
            .nth(n, |c| matches!(c, MpvCommand::LoadFile { .. }))
            .expect("loadfile sent");
        self.reply(id, json!({ "playlist_entry_id": entry }));
    }

    fn loadfile_err(&mut self, n: usize) {
        let id = self
            .nth(n, |c| matches!(c, MpvCommand::LoadFile { .. }))
            .expect("loadfile sent");
        self.mpv_message(&json!({"request_id": id, "error": "invalid parameter"}).to_string());
    }

    /// mpv's reply to the `n`th `playlist-clear`, if it was sent.
    fn clear_ok(&mut self, n: usize) {
        if let Some(id) = self.nth(n, |c| *c == MpvCommand::PlaylistClear) {
            self.reply(id, json!(null));
        }
    }

    /// mpv's reply to the `n`th `loadfile … append`, if it was sent.
    fn append_ok(&mut self, n: usize, entry: i64) {
        if let Some(id) = self.nth(n, |c| matches!(c, MpvCommand::AppendFile { .. })) {
            self.reply(id, json!({ "playlist_entry_id": entry }));
        }
    }

    /// mpv's answer to the `n`th `idle-active` query, if it was asked.
    fn check(&mut self, n: usize, idle: bool) {
        let query = MpvCommand::GetProperty {
            name: IDLE_PROPERTY,
        };
        if let Some(id) = self.nth(n, |c| *c == query) {
            self.reply(id, json!(idle));
        }
    }

    /// mpv answers every lookahead request still unanswered, giving new
    /// entries ids above any used, until nothing more is sent.
    fn settle_lookahead(&mut self) {
        let mut entry = 1000;
        loop {
            let open = self.requests(|c| {
                matches!(c, MpvCommand::PlaylistClear | MpvCommand::AppendFile { .. })
            });
            let Some(request) = open
                .into_iter()
                .find(|r| !self.answered.contains(&r.request_id))
                .cloned()
            else {
                return;
            };
            if request.command == MpvCommand::PlaylistClear {
                self.reply(request.request_id, json!(null));
            } else {
                entry += 1;
                self.reply(request.request_id, json!({ "playlist_entry_id": entry }));
            }
        }
    }

    fn checks_sent(&self) -> usize {
        let query = MpvCommand::GetProperty {
            name: IDLE_PROPERTY,
        };
        self.requests(|c| *c == query).len()
    }

    fn start_file(&mut self, entry: i64) {
        self.mpv_message(&json!({"event": "start-file", "playlist_entry_id": entry}).to_string());
    }

    /// mpv reports the playback position (`time-pos`).
    fn position(&mut self, seconds: f64) {
        self.mpv_message(
            &json!({"event": "property-change", "id": 1, "name": "time-pos", "data": seconds})
                .to_string(),
        );
    }

    fn file_loaded(&mut self) {
        self.mpv_message(r#"{"event": "file-loaded"}"#);
    }

    fn end_file(&mut self, entry: i64, reason: &str) {
        let mut v = json!({"event": "end-file", "reason": reason, "playlist_entry_id": entry});
        if reason == "error" {
            v["file_error"] = "loading failed".into();
        }
        self.mpv_message(&v.to_string());
    }

    /// Completes load `n` normally as entry `entry`: reply, start-file
    /// and file-loaded.
    fn complete_load(&mut self, n: usize, entry: i64) {
        self.loadfile_ok(n, entry);
        self.start_file(entry);
        self.file_loaded();
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::LoadfileOk(n, entry) => self.loadfile_ok(n, entry),
            Msg::LoadfileErr(n) => self.loadfile_err(n),
            Msg::Clear(n) => self.clear_ok(n),
            Msg::AppendOk(n, entry) => self.append_ok(n, entry),
            Msg::Check(n, idle) => self.check(n, idle),
            Msg::Start(entry) => self.start_file(entry),
            Msg::Loaded => self.file_loaded(),
            Msg::End(entry, reason) => self.end_file(entry, reason),
            Msg::Pos(seconds) => self.position(seconds),
            Msg::RemoveNext => self.remove_next(),
            Msg::PlayNext(name) => self.play_next(name),
            Msg::Play(index) => self.play(index),
            Msg::ClearQueue => self.clear_queue(),
            Msg::Settle => self.settle_lookahead(),
        }
    }

    fn stops_sent(&self) -> usize {
        self.requests(|c| *c == MpvCommand::Stop).len()
    }

    // ----- assertions ---------------------------------------------------

    fn current_index(&self) -> Option<usize> {
        let current = self.player.queue().current()?;
        self.ids.iter().position(|id| *id == current)
    }

    fn current_title(&self) -> String {
        self.player
            .queue()
            .current_entry()
            .unwrap()
            .track
            .title
            .clone()
    }

    fn heard_title(&self) -> String {
        self.player.playing_entry().unwrap().track.title.clone()
    }

    fn assert_outcome(&self, current: usize, loads: usize) {
        assert_eq!(self.current_index(), Some(current), "queue position");
        assert_eq!(self.loads.len(), loads, "number of Load commands");
    }
}

fn track(name: &str) -> Track {
    Track::local(format!("/m/{name}.wav").into(), TrackMetadata::default())
}

fn path(name: &str) -> String {
    PathBuf::from(format!("/m/{name}.wav"))
        .to_string_lossy()
        .into_owned()
}

/// One scripted mpv message or user intent. `Loaded` (`file-loaded`)
/// carries no entry id; mpv attributes it to the preceding `start-file`.
/// Replies to requests Coda did not send in that run are skipped (the
/// order of events decided that the request never went out).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Msg {
    /// Reply to the nth `loadfile … replace`, naming the entry.
    LoadfileOk(usize, i64),
    LoadfileErr(usize),
    /// Reply to the nth `playlist-clear`.
    Clear(usize),
    /// Reply to the nth `loadfile … append`, naming the entry.
    AppendOk(usize, i64),
    /// Answer to the nth `idle-active` query.
    Check(usize, bool),
    Start(i64),
    Loaded,
    End(i64, &'static str),
    /// `time-pos` in seconds.
    Pos(f64),
    /// The user removes the entry after the current one.
    RemoveNext,
    /// The user puts a new track next (`A`).
    PlayNext(&'static str),
    /// The user plays the queue entry at this index (Enter).
    Play(usize),
    /// The user clears the queue (`C C`).
    ClearQueue,
    /// mpv answers every lookahead request still open.
    Settle,
}

// ----- rapid skipping ------------------------------------------------------

#[test]
fn rapid_skipping_while_playing() {
    let mut sim = Sim::new(&["A", "B", "C", "D", "E"]);
    sim.play(0);
    sim.complete_load(0, 1);
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);

    // Three `n` presses before mpv replies to anything.
    sim.press_n();
    sim.press_n();
    sim.press_n();
    assert_eq!(sim.loads.len(), 4);
    // B is in flight; C was superseded by D in the backend queue.
    assert_eq!(sim.sent_paths(), [path("A"), path("B")]);

    // mpv processes B: its reply names the entry; only now does D go
    // out; C never reaches mpv.
    sim.loadfile_ok(1, 2);
    assert_eq!(sim.sent_paths(), [path("A"), path("B"), path("D")]);
    sim.end_file(1, "stop");
    sim.start_file(2);
    sim.file_loaded();
    sim.loadfile_ok(2, 3);
    sim.end_file(2, "stop");
    sim.start_file(3);
    sim.file_loaded();

    sim.assert_outcome(3, 4);
    assert_eq!(sim.advances, 0);
    assert_eq!(
        sim.started_current,
        [sim.loads[0], sim.loads[3]],
        "only A then D started for the player"
    );
    assert_eq!(sim.started_stale, 1, "B's start was stale");
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);

    // D finishes normally: exactly one advance, to E.
    sim.end_file(3, "eof");
    sim.assert_outcome(4, 5);
    assert_eq!(sim.advances, 1);
}

#[test]
fn rapid_skipping_before_first_reply() {
    let mut sim = Sim::new(&["A", "B", "C", "D"]);
    sim.play(0);
    sim.press_n();
    sim.press_n();
    sim.press_n();
    assert_eq!(sim.loads.len(), 4);
    assert_eq!(sim.sent_paths(), [path("A")]);

    // A's events, including its start, arrive in various positions.
    sim.start_file(1);
    sim.loadfile_ok(0, 1);
    sim.file_loaded();
    assert_eq!(sim.sent_paths(), [path("A"), path("D")]);
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "stop");
    sim.start_file(2);
    sim.file_loaded();

    sim.assert_outcome(3, 4);
    assert_eq!(
        sim.started_current,
        [sim.loads[3]],
        "exactly one track plays for the player"
    );
    assert_eq!(sim.advances, 0);
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);
}

#[test]
fn rapid_skipping_to_the_end_of_the_queue() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.complete_load(0, 1);
    sim.press_n();
    sim.press_n(); // at end: no-op
    sim.press_n();
    assert_eq!(sim.loads.len(), 2);
    assert_eq!(sim.messages, ["End of queue", "End of queue"]);
    sim.end_file(1, "eof"); // A also finished at the same moment
    sim.loadfile_ok(1, 2);
    sim.start_file(2);
    sim.file_loaded();
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 0);
}

// ----- replacement during playback -----------------------------------------

#[test]
fn replacement_old_end_file_never_advances() {
    // The old entry's end-file with reason stop or eof, before or after the
    // new entry's loadfile reply.
    for old_reason in ["stop", "eof"] {
        for old_before_reply in [true, false] {
            let mut sim = Sim::new(&["A", "B", "C"]);
            sim.play(0);
            sim.complete_load(0, 1);
            sim.press_n();
            if old_before_reply {
                sim.end_file(1, old_reason);
                sim.loadfile_ok(1, 2);
            } else {
                sim.loadfile_ok(1, 2);
                sim.end_file(1, old_reason);
            }
            sim.start_file(2);
            sim.file_loaded();
            let label = format!("{old_reason}, before reply: {old_before_reply}");
            assert_eq!(sim.current_index(), Some(1), "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.advances, 0, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
        }
    }
}

#[test]
fn started_waits_for_file_loaded() {
    let mut sim = Sim::new(&["A"]);
    sim.play(0);
    sim.loadfile_ok(0, 1);
    sim.start_file(1);
    assert_eq!(sim.player.status(), PlaybackStatus::Loading);
    assert!(
        sim.player.seek_relative(5.0).is_empty(),
        "no seek while opening"
    );
    sim.file_loaded();
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);
    assert_eq!(
        sim.player.seek_relative(5.0),
        [PlayerCommand::SeekRelative(5.0)]
    );
}

#[test]
fn enter_on_library_track_during_playback() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.complete_load(0, 1);
    let x = Track::local("/m/X.wav".into(), TrackMetadata::default());
    let commands = sim.player.play_context(vec![x], 0);
    sim.commands(commands);
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "eof");
    sim.start_file(2);
    sim.file_loaded();
    let titles: Vec<_> = sim
        .player
        .queue()
        .entries()
        .iter()
        .map(|e| e.track.title.as_str())
        .collect();
    assert_eq!(titles, ["X"], "Enter replaced the queue");
    assert_eq!(sim.player.queue().current_entry().unwrap().track.title, "X");
    assert_eq!(sim.loads.len(), 2);
    assert_eq!(sim.advances, 0);
}

// ----- very short files ----------------------------------------------------

#[test]
fn very_short_file_events_before_loadfile_reply() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.start_file(1);
    sim.file_loaded();
    sim.end_file(1, "eof");
    assert_eq!(sim.loads.len(), 1, "buffered until the id is known");
    sim.loadfile_ok(0, 1);
    // Replayed: Started then EOF, which advances exactly once.
    assert_eq!(sim.started_current, [sim.loads[0]]);
    assert_eq!(sim.advances, 1);
    sim.assert_outcome(1, 2);
    sim.complete_load(1, 2);
    sim.end_file(2, "eof");
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 2);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
}

#[test]
fn queue_of_very_short_files_plays_through_once() {
    let mut sim = Sim::new(&["A", "B", "C", "D"]);
    sim.play(0);
    for n in 0..4 {
        let entry = n as i64 + 1;
        sim.start_file(entry);
        sim.file_loaded();
        sim.end_file(entry, "eof");
        sim.loadfile_ok(n, entry);
    }
    sim.assert_outcome(3, 4);
    assert_eq!(sim.advances, 4);
    assert_eq!(sim.started_current, sim.loads);
    assert_eq!(
        sim.sent_paths(),
        [path("A"), path("B"), path("C"), path("D")]
    );
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
}

// ----- failed loads --------------------------------------------------------

#[test]
fn failed_load_end_file_error_before_loadfile_reply() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.start_file(1);
    sim.end_file(1, "error");
    sim.loadfile_ok(0, 1);
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 1);
    assert!(sim.started_current.is_empty());
    assert_eq!(sim.messages, ["Could not play A: loading failed"]);
}

#[test]
fn failed_loadfile_reply_advances_once() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.loadfile_err(0);
    // An entry Coda never mapped ends: ignored.
    sim.end_file(99, "eof");
    assert_eq!(sim.advances, 1);
    sim.assert_outcome(1, 2);
    assert_eq!(sim.messages.len(), 1);
    assert!(sim.messages[0].starts_with("Could not play A"));
}

#[test]
fn failed_load_at_end_of_queue_stops() {
    let mut sim = Sim::new(&["A"]);
    sim.play(0);
    sim.loadfile_ok(0, 1);
    sim.end_file(1, "error");
    sim.assert_outcome(0, 1);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert!(sim.sent.iter().any(|r| r.command == MpvCommand::Stop));
}

fn reject_next(sim: &mut Sim, name: &str) {
    sim.missing.push(path(name));
    sim.press_n();
}

#[test]
fn missing_file_mid_skip_does_not_leave_old_track_playing() {
    // A plays; the user skips to B whose file vanished (rejected by the
    // driver without contacting mpv). B is last, so the player stops mpv.
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.complete_load(0, 1);
    reject_next(&mut sim, "B");
    assert_eq!(sim.advances, 1);
    sim.assert_outcome(1, 2);
    assert_eq!(sim.messages, ["Could not play B: file not found"]);
    assert_eq!(sim.sent.last().unwrap().command, MpvCommand::Stop);
    // A's resulting end-file is stale.
    sim.end_file(1, "stop");
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert_eq!(sim.advances, 1);
}

#[test]
fn missing_file_while_another_load_is_in_flight() {
    // A plays; `n` sends B (in flight); `n` again targets C, whose file is
    // gone. C is last, so the player stops; the stop reaches mpv after B's
    // load, so B does not keep playing.
    let mut sim = Sim::new(&["A", "B", "C"]);
    sim.play(0);
    sim.complete_load(0, 1);
    sim.press_n();
    reject_next(&mut sim, "C");
    let loadfile_b = sim
        .sent
        .iter()
        .position(|r| matches!(&r.command, MpvCommand::LoadFile { path: p } if *p == path("B")))
        .unwrap();
    let stop = sim
        .sent
        .iter()
        .position(|r| r.command == MpvCommand::Stop)
        .expect("stop sent");
    assert!(stop > loadfile_b);
    // B's and A's events are all stale now.
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "stop");
    sim.start_file(2);
    sim.file_loaded();
    sim.end_file(2, "stop");
    sim.assert_outcome(2, 3);
    assert_eq!(sim.advances, 1);
    assert_eq!(sim.sent_paths(), [path("A"), path("B")]);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
}

// ----- exhaustive interleavings --------------------------------------------

/// All orders of `0..n` in which every `(a, b)` pair has `a` before `b`.
fn orders(n: usize, before: &[(usize, usize)]) -> Vec<Vec<usize>> {
    fn permute(rest: &mut Vec<usize>, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if rest.is_empty() {
            out.push(prefix.clone());
            return;
        }
        for i in 0..rest.len() {
            let item = rest.remove(i);
            prefix.push(item);
            permute(rest, prefix, out);
            prefix.pop();
            rest.insert(i, item);
        }
    }
    let mut out = Vec::new();
    permute(&mut (0..n).collect(), &mut Vec::new(), &mut out);
    out.retain(|order| {
        let pos = |x| order.iter().position(|y| *y == x).unwrap();
        before.iter().all(|&(a, b)| pos(a) < pos(b))
    });
    out
}

/// A group of messages that may arrive in any order allowed by `before`.
/// Groups are played one after another.
struct Group {
    msgs: Vec<Msg>,
    before: Vec<(usize, usize)>,
}

fn group(msgs: &[Msg], before: &[(usize, usize)]) -> Group {
    Group {
        msgs: msgs.to_vec(),
        before: before.to_vec(),
    }
}

/// Runs `setup`, then every combination of group orders, calling `check`
/// on the final state. Returns the number of runs.
fn exhaust(setup: &dyn Fn() -> Sim, groups: &[Group], check: &dyn Fn(&Sim, &str)) -> usize {
    let per_group: Vec<Vec<Vec<usize>>> = groups
        .iter()
        .map(|g| orders(g.msgs.len(), &g.before))
        .collect();
    let mut runs = 0;
    let mut choice = vec![0usize; groups.len()];
    loop {
        let mut sim = setup();
        let mut script = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            for &i in &per_group[g][choice[g]] {
                script.push(group.msgs[i]);
                sim.apply(group.msgs[i]);
            }
        }
        check(&sim, &format!("{script:?}"));
        runs += 1;
        // Advance the mixed-radix counter over group orders.
        let mut g = groups.len();
        loop {
            if g == 0 {
                return runs;
            }
            g -= 1;
            choice[g] += 1;
            if choice[g] < per_group[g].len() {
                break;
            }
            choice[g] = 0;
        }
    }
}

/// A is playing as entry 1; `n` loads `names[1]` (load index 1). mpv never
/// answers the lookahead's requests here, so nothing follows A in mpv.
fn playing_then_next(names: &'static [&'static str]) -> impl Fn() -> Sim {
    move || {
        let mut sim = Sim::new(names);
        sim.play(0);
        sim.complete_load(0, 1);
        sim.press_n();
        sim
    }
}

#[test]
fn every_replacement_interleaving_advances_exactly_once_or_never() {
    use Msg::*;
    let setup = playing_then_next(&["A", "B", "C"]);
    let mut runs = 0;
    for old in ["stop", "eof"] {
        // New entry ends (eof after loading, error with or without loading).
        for (new_end, loaded) in [("eof", true), ("error", true), ("error", false)] {
            let mut msgs = vec![LoadfileOk(1, 2), Start(2), End(2, new_end), End(1, old)];
            let mut before = vec![(1, 2)];
            if loaded {
                msgs.push(Loaded);
                before.extend([(1, 4), (4, 2)]);
            }
            runs += exhaust(&setup, &[group(&msgs, &before)], &|sim, label| {
                assert_eq!(sim.advances, 1, "{label}");
                assert_eq!(sim.current_index(), Some(2), "{label}");
                assert_eq!(sim.loads.len(), 3, "{label}");
                assert_eq!(
                    sim.sent_paths(),
                    [path("A"), path("B"), path("C")],
                    "{label}"
                );
                assert_eq!(sim.player.status(), PlaybackStatus::Loading, "{label}");
                assert_eq!(
                    sim.messages.len(),
                    usize::from(new_end == "error"),
                    "{label}"
                );
                let expected_started = if loaded { 2 } else { 1 };
                assert_eq!(sim.started_current.len(), expected_started, "{label}");
                assert_eq!(sim.started_stale, 0, "{label}");
                assert_eq!(sim.stops_sent(), 0, "{label}");
            });
        }
        // New entry keeps playing.
        let msgs = [LoadfileOk(1, 2), Start(2), Loaded, End(1, old)];
        runs += exhaust(&setup, &[group(&msgs, &[(1, 2)])], &|sim, label| {
            assert_eq!(sim.advances, 0, "{label}");
            assert_eq!(sim.current_index(), Some(1), "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[1]], "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.stops_sent(), 0, "{label}");
        });
    }
    assert_eq!(runs, 2 * (20 + 20 + 12 + 12));
}

#[test]
fn every_interleaving_of_collapsed_skips_ends_on_the_last_request() {
    use Msg::*;
    // A playing; three `n` presses: B goes out, C and D collapse into D.
    let setup = || {
        let mut sim = playing_then_next(&["A", "B", "C", "D"])();
        sim.press_n();
        sim.press_n();
        assert_eq!(sim.loads.len(), 4);
        sim
    };
    let mut runs = 0;
    for old in ["stop", "eof"] {
        let first = group(
            &[LoadfileOk(1, 2), Start(2), Loaded, End(1, old)],
            &[(1, 2)],
        );
        // D is sent once B's entry is known.
        let second = group(
            &[LoadfileOk(2, 3), End(2, "stop"), Start(3), Loaded],
            &[(2, 3)],
        );
        runs += exhaust(&setup, &[first, second], &|sim, label| {
            assert_eq!(sim.loads.len(), 4, "{label}");
            assert_eq!(
                sim.sent_paths(),
                [path("A"), path("B"), path("D")],
                "{label}"
            );
            assert_eq!(sim.current_index(), Some(3), "{label}");
            assert_eq!(sim.advances, 0, "{label}");
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[3]], "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.stops_sent(), 0, "{label}");
        });
    }
    assert_eq!(runs, 2 * 12 * 12);
}

#[test]
fn every_interleaving_of_stop_while_a_load_is_in_flight() {
    use Msg::*;
    // A playing; `n` sends B; removing B (current, last) stops playback
    // while B's load is still in flight.
    let setup = || {
        let mut sim = playing_then_next(&["A", "B"])();
        let id = sim.ids[1];
        let commands = sim.player.remove(id);
        assert_eq!(commands, [PlayerCommand::Stop]);
        sim.commands(commands);
        sim
    };
    let mut runs = 0;
    for old in ["stop", "eof"] {
        for loaded in [false, true] {
            let mut msgs = vec![LoadfileOk(1, 2), Start(2), End(2, "stop"), End(1, old)];
            let mut before = vec![(1, 2)];
            if loaded {
                msgs.push(Loaded);
                before.extend([(1, 4), (4, 2)]);
            }
            runs += exhaust(&setup, &[group(&msgs, &before)], &|sim, label| {
                assert_eq!(sim.loads.len(), 2, "{label}");
                assert_eq!(sim.advances, 0, "{label}");
                assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
                assert_eq!(sim.player.queue().len(), 1, "{label}");
                assert_eq!(sim.current_index(), Some(0), "{label}");
                assert_eq!(sim.stops_sent(), 1, "{label}: no extra stop");
                assert_eq!(sim.started_current, [sim.loads[0]], "{label}");
                assert_eq!(sim.sent_paths(), [path("A"), path("B")], "{label}");
            });
        }
    }
    assert_eq!(runs, 2 * (12 + 20));
}

#[test]
fn every_interleaving_after_a_failed_loadfile_reply() {
    use Msg::*;
    let setup = playing_then_next(&["A", "B", "C"]);
    let check = |sim: &Sim, label: &str| {
        assert_eq!(sim.advances, 1, "{label}");
        assert_eq!(sim.messages.len(), 1, "{label}");
        assert!(sim.messages[0].starts_with("Could not play B"), "{label}");
        assert_eq!(sim.current_index(), Some(2), "{label}");
        assert_eq!(sim.loads.len(), 3, "{label}");
        assert_eq!(
            sim.sent_paths(),
            [path("A"), path("B"), path("C")],
            "{label}"
        );
        assert_eq!(sim.started_current, [sim.loads[0], sim.loads[2]], "{label}");
        assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
    };
    // C is sent as soon as B's failure is known. A ends either by finishing
    // (eof) early or by C replacing it.
    let mut runs = exhaust(
        &setup,
        &[
            group(&[LoadfileErr(1), End(1, "eof")], &[]),
            group(&[LoadfileOk(2, 2), Start(2), Loaded], &[(1, 2)]),
        ],
        &check,
    );
    runs += exhaust(
        &setup,
        &[
            group(&[LoadfileErr(1)], &[]),
            group(
                &[LoadfileOk(2, 2), End(1, "stop"), Start(2), Loaded],
                &[(2, 3)],
            ),
        ],
        &check,
    );
    assert_eq!(runs, 2 * 3 + 12);
}

#[test]
fn every_interleaving_of_a_redirecting_file() {
    use Msg::*;
    // B is a "playlist in disguise": mpv ends it with `redirect` and
    // starts an inserted entry 3 that Coda never requested.
    let first = |old| {
        group(
            &[
                LoadfileOk(1, 2),
                Start(2),
                End(2, "redirect"),
                Start(3),
                Loaded,
                End(1, old),
            ],
            &[(1, 2), (2, 3), (3, 4)],
        )
    };
    let mut runs = 0;
    for old in ["stop", "eof"] {
        // With a next entry: the redirect is an error and C replaces it.
        let setup = playing_then_next(&["A", "B", "C"]);
        let second = group(
            &[LoadfileOk(2, 5), End(3, "stop"), Start(5), Loaded],
            &[(2, 3)],
        );
        runs += exhaust(&setup, &[first(old), second], &|sim, label| {
            assert_eq!(sim.advances, 1, "{label}");
            assert_eq!(
                sim.messages,
                ["Could not play B: not an audio file (playlist or directory)"],
                "{label}"
            );
            assert_eq!(sim.current_index(), Some(2), "{label}");
            assert_eq!(sim.loads.len(), 3, "{label}");
            assert_eq!(
                sim.sent_paths(),
                [path("A"), path("B"), path("C")],
                "{label}"
            );
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[2]], "{label}");
            assert_eq!(sim.started_stale, 0, "{label}: inserted entry never mapped");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
        });
        // At the end of the queue: the player stops, and the inserted entry
        // is stopped too if it starts after that.
        let setup = playing_then_next(&["A", "B"]);
        runs += exhaust(&setup, &[first(old)], &|sim, label| {
            assert_eq!(sim.advances, 1, "{label}");
            assert_eq!(sim.current_index(), Some(1), "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
            assert!(sim.stops_sent() >= 1, "{label}");
            assert_eq!(sim.started_current, [sim.loads[0]], "{label}");
            assert_eq!(sim.started_stale, 0, "{label}");
        });
    }
    assert_eq!(runs, 2 * (30 * 12 + 30));
}

// ----- repeat --------------------------------------------------------------

fn repeating(names: &[&str], mode: RepeatMode) -> Sim {
    let mut sim = Sim::new(names);
    sim.player.set_repeat(mode);
    sim
}

#[test]
fn repeat_one_reload_is_never_confused_with_the_old_end_file() {
    use Msg::*;
    // A plays as entry 1 and ends: the player reloads A (load index 1).
    let setup = || {
        let mut sim = repeating(&["A", "B"], RepeatMode::One);
        sim.play(0);
        sim.complete_load(0, 1);
        sim.position(2.0);
        sim.end_file(1, "eof");
        assert_eq!(sim.loads.len(), 2);
        sim
    };
    let mut runs = 0;
    // The reload plays; a duplicate end-file for the old entry (stop or
    // eof) arrives anywhere and must be stale.
    for old in ["stop", "eof"] {
        let msgs = [LoadfileOk(1, 2), Start(2), Loaded, End(1, old)];
        runs += exhaust(&setup, &[group(&msgs, &[(1, 2)])], &|sim, label| {
            assert_eq!(sim.advances, 1, "{label}");
            assert_eq!(sim.current_index(), Some(0), "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.sent_paths(), [path("A"), path("A")], "{label}");
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[1]], "{label}");
            assert_eq!(sim.started_stale, 0, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.stops_sent(), 0, "{label}");
        });
    }
    // The reload plays (its position passes a second) and ends: repeated
    // once more.
    let msgs = [LoadfileOk(1, 2), Start(2), Loaded, Pos(2.0), End(2, "eof")];
    runs += exhaust(
        &setup,
        &[group(&msgs, &[(1, 2), (2, 3), (0, 3), (3, 4)])],
        &|sim, label| {
            assert_eq!(sim.advances, 2, "{label}");
            assert_eq!(sim.current_index(), Some(0), "{label}");
            assert_eq!(sim.loads.len(), 3, "{label}");
            assert_eq!(
                sim.sent_paths(),
                [path("A"), path("A"), path("A")],
                "{label}"
            );
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[1]], "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Loading, "{label}");
        },
    );
    // The reload ends without playing (e.g. the file was truncated), in any
    // order, even before its entry id is known: never reloaded again; the
    // queue moves on to B.
    let msgs = [LoadfileOk(1, 2), Start(2), Loaded, End(2, "eof")];
    runs += exhaust(&setup, &[group(&msgs, &[(1, 2), (2, 3)])], &|sim, label| {
        assert_eq!(sim.advances, 2, "{label}");
        assert_eq!(sim.current_index(), Some(1), "{label}");
        assert_eq!(sim.loads.len(), 3, "{label}");
        assert_eq!(
            sim.sent_paths(),
            [path("A"), path("A"), path("B")],
            "{label}"
        );
    });
    assert_eq!(runs, 2 * 12 + 3 + 4);
}

#[test]
fn next_during_a_repeat_one_reload_wins() {
    let mut sim = repeating(&["A", "B"], RepeatMode::One);
    sim.play(0);
    sim.complete_load(0, 1);
    sim.position(2.0);
    sim.end_file(1, "eof");
    // `n` while the reload is in flight: B collapses behind it.
    sim.press_n();
    assert_eq!(sim.loads.len(), 3);
    sim.loadfile_ok(1, 2);
    sim.start_file(2);
    sim.file_loaded();
    sim.loadfile_ok(2, 3);
    sim.end_file(2, "stop");
    sim.start_file(3);
    sim.file_loaded();
    sim.assert_outcome(1, 3);
    assert_eq!(sim.advances, 1);
    assert_eq!(sim.started_stale, 1, "the reload's start was stale");
    assert_eq!(sim.sent_paths(), [path("A"), path("A"), path("B")]);
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);
}

#[test]
fn repeat_all_wraps_and_ignores_the_last_entrys_old_end_file() {
    let mut sim = repeating(&["A", "B"], RepeatMode::All);
    sim.play(1);
    sim.complete_load(0, 1);
    sim.position(2.0);
    sim.end_file(1, "eof");
    sim.assert_outcome(0, 2);
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "stop");
    sim.start_file(2);
    sim.file_loaded();
    sim.end_file(1, "eof");
    sim.assert_outcome(0, 2);
    assert_eq!(sim.advances, 1);
    assert_eq!(sim.player.status(), PlaybackStatus::Playing);
    // `n` at the end wraps as well.
    sim.press_n();
    sim.complete_load(2, 3);
    sim.press_n();
    sim.assert_outcome(0, 4);
    assert_eq!(
        sim.sent_paths(),
        [path("B"), path("A"), path("B"), path("A")]
    );
}

/// Every loadfile fails; returns the sim once nothing more is sent.
fn fail_every_load(mode: RepeatMode) -> Sim {
    let mut sim = repeating(&["A", "B", "C"], mode);
    sim.play(1);
    let mut n = 0;
    while n < sim.sent_paths().len() {
        assert!(n < 30, "did not terminate");
        sim.loadfile_err(n);
        n += 1;
    }
    sim
}

#[test]
fn all_failing_queue_terminates_under_repeat() {
    // All: B, C, A fail (one full round), then stop.
    let sim = fail_every_load(RepeatMode::All);
    assert_eq!(sim.sent_paths(), [path("B"), path("C"), path("A")]);
    assert_eq!(sim.advances, 3);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert_eq!(sim.stops_sent(), 1);
    assert_eq!(
        sim.messages.last().map(String::as_str),
        Some(NO_PLAYABLE_TRACKS)
    );
    assert_eq!(sim.current_index(), Some(0));
    // One: errors advance without wrapping, so the end of the queue stops.
    let sim = fail_every_load(RepeatMode::One);
    assert_eq!(sim.sent_paths(), [path("B"), path("C")]);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert_eq!(sim.stops_sent(), 1);
    assert_eq!(sim.current_index(), Some(2));
}

#[test]
fn files_that_end_at_once_never_loop_under_repeat() {
    for mode in [RepeatMode::One, RepeatMode::All] {
        let mut sim = repeating(&["A", "B", "C"], mode);
        sim.play(0);
        let mut n = 0;
        while n < sim.sent_paths().len() {
            assert!(n < 30, "{mode:?}: did not terminate");
            let entry = n as i64 + 1;
            sim.complete_load(n, entry);
            sim.position(0.2);
            sim.end_file(entry, "eof");
            n += 1;
        }
        assert_eq!(
            sim.sent_paths(),
            [path("A"), path("B"), path("C")],
            "{mode:?}"
        );
        assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{mode:?}");
        assert_eq!(
            sim.messages.last().map(String::as_str) == Some(NO_PLAYABLE_TRACKS),
            mode == RepeatMode::All,
            "{mode:?}: {:?}",
            sim.messages
        );
    }
}

// ----- gapless lookahead (M8) ------------------------------------------------

/// `names[0]` (A) plays as entry 1, and mpv holds a copy of `names[1]` (B)
/// as entry 2: its set-next requests were answered.
fn playing_with_next(names: &'static [&'static str]) -> impl Fn() -> Sim {
    move || {
        let mut sim = Sim::new(names);
        sim.play(0);
        sim.complete_load(0, 1);
        assert_eq!(sim.appended_paths(), [path(names[1])]);
        sim.clear_ok(0);
        sim.append_ok(0, 2);
        sim.position(0.5);
        sim
    }
}

#[test]
fn the_next_entry_is_appended_once_the_current_one_is_in_mpv() {
    let mut sim = Sim::new(&["A", "B", "C"]);
    sim.play(0);
    assert!(sim.appended_paths().is_empty(), "A is not in mpv yet");
    sim.loadfile_ok(0, 1);
    assert_eq!(sim.appended_paths(), [path("B")]);
    let clear = sim.nth(0, |c| *c == MpvCommand::PlaylistClear).unwrap();
    let append = sim
        .nth(0, |c| matches!(c, MpvCommand::AppendFile { .. }))
        .unwrap();
    assert!(clear < append, "the clear goes first");
    // Never by index: no `playlist-remove` or `playlist-move` exists in the
    // backend's vocabulary, and nothing but the clear and the append.
    assert_eq!(
        sim.requests(|c| matches!(c, MpvCommand::PlaylistClear | MpvCommand::AppendFile { .. }))
            .len(),
        2
    );
}

#[test]
fn every_interleaving_of_set_next_replies_around_the_end() {
    use Msg::*;
    let setup = || {
        let mut sim = Sim::new(&["A", "B", "C"]);
        sim.play(0);
        sim.complete_load(0, 1);
        sim.position(0.5);
        sim
    };
    let mut runs = 0;
    // The clear and the append ran before A ended: their replies come
    // before A's end-file, and mpv goes on to B without a gap. mpv's answer
    // to the check may come before B's start (it was written first).
    let took_it = group(
        &[Clear(0), AppendOk(0, 2), End(1, "eof")],
        &[(0, 1), (1, 2)],
    );
    let went_on = group(
        &[Start(2), Loaded, Pos(-0.2), Pos(0.1), Check(0, false)],
        &[(0, 1), (1, 2), (2, 3)],
    );
    runs += exhaust(&setup, &[took_it, went_on], &|sim, label| {
        assert_eq!(sim.gapless, 1, "{label}");
        assert_eq!(sim.corrections, 0, "{label}");
        assert_eq!(sim.advances, 0, "{label}");
        assert_eq!(sim.loads.len(), 1, "{label}: no load for B");
        assert_eq!(sim.current_index(), Some(1), "{label}");
        assert_eq!(sim.started_current.len(), 2, "{label}");
        assert_eq!(sim.heard_title(), "B", "{label}");
        assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
        assert_eq!(sim.stops_sent(), 0, "{label}");
        // What follows B is held next.
        assert_eq!(sim.appended_paths(), [path("B"), path("C")], "{label}");
    });
    // The append ran after A ended (its reply comes after A's end-file;
    // the clear may have run either side): mpv stopped, and B is loaded
    // as before M8.
    let too_late = group(
        &[Clear(0), End(1, "eof"), AppendOk(0, 2)],
        &[(0, 2), (1, 2)],
    );
    let loaded = group(&[LoadfileOk(1, 3), Start(3), Loaded], &[(1, 2)]);
    runs += exhaust(&setup, &[too_late, loaded], &|sim, label| {
        assert_eq!(sim.gapless, 0, "{label}");
        assert_eq!(sim.advances, 1, "{label}");
        assert_eq!(sim.loads.len(), 2, "{label}");
        assert_eq!(sim.sent_paths(), [path("A"), path("B")], "{label}");
        assert_eq!(sim.checks_sent(), 0, "{label}: nothing could follow A");
        assert_eq!(sim.current_index(), Some(1), "{label}");
        assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
    });
    assert_eq!(runs, 5 + 2 * 3);
}

#[test]
fn started_follows_the_audible_marker_in_every_order() {
    use Msg::*;
    // B's `Started` (and so the now-playing bar and scrobbling) waits until
    // B is heard: the first position above zero after its file-loaded, or
    // its end if that comes first.
    let setup = || {
        let mut sim = playing_with_next(&["A", "B"])();
        sim.end_file(1, "eof");
        sim.start_file(2);
        assert_eq!(sim.gapless, 1);
        sim
    };
    let mut runs = 0;
    for positions in [
        &[Pos(0.0), Pos(0.112)][..],
        &[Pos(-0.33), Pos(-0.1), Pos(0.05)],
    ] {
        let mut msgs = vec![Loaded];
        msgs.extend_from_slice(positions);
        let before: Vec<_> = (0..msgs.len() - 1).map(|i| (i, i + 1)).collect();
        runs += exhaust(&setup, &[group(&msgs, &before)], &|sim, label| {
            assert_eq!(sim.started_current.len(), 2, "{label}");
            assert_eq!(sim.heard_title(), "B", "{label}");
        });
        // Before the marker, A is still what is heard.
        let mut sim = setup();
        sim.file_loaded();
        for &p in &positions[..positions.len() - 1] {
            sim.apply(p);
        }
        assert_eq!(sim.started_current.len(), 1);
        assert_eq!(sim.heard_title(), "A");
        assert_eq!(sim.player.status(), PlaybackStatus::Playing);
        assert_eq!(sim.current_title(), "B");
    }
    // A very short B ends before any position above zero.
    let mut sim = setup();
    sim.file_loaded();
    sim.position(0.0);
    sim.end_file(2, "eof");
    assert_eq!(sim.started_current.len(), 2);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert_eq!(runs, 2);
}

/// What a queue edit (`edit`) made next, starting from A playing with B
/// held next in mpv as entry 2; the new next track gets entry 3.
fn edit_cases() -> [(Msg, &'static str); 2] {
    [(Msg::RemoveNext, "C"), (Msg::PlayNext("X"), "X")]
}

#[test]
fn every_queue_edit_mpv_took_before_the_end_plays_without_a_gap() {
    use Msg::*;
    let mut runs = 0;
    for (edit, next) in edit_cases() {
        let setup = playing_with_next(&["A", "B", "C"]);
        // The clear and append ran before A ended, so their replies come
        // first; then mpv starts the new next entry.
        let took_it = group(
            &[edit, Clear(1), AppendOk(1, 3), End(1, "eof")],
            &[(0, 1), (1, 2), (2, 3)],
        );
        let went_on = group(
            &[Start(3), Loaded, Pos(0.1), Check(0, false)],
            &[(0, 1), (1, 2)],
        );
        runs += exhaust(&setup, &[took_it, went_on], &|sim, label| {
            assert_eq!(sim.gapless, 1, "{label}");
            assert_eq!(sim.corrections, 0, "{label}");
            assert_eq!(sim.loads.len(), 1, "{label}");
            assert_eq!(sim.current_title(), next, "{label}");
            assert_eq!(sim.heard_title(), next, "{label}");
            assert_eq!(sim.started_current.len(), 2, "{label}");
            assert_eq!(sim.started_stale, 0, "{label}: B never started");
            assert_eq!(sim.stops_sent(), 0, "{label}");
        });
    }
    assert_eq!(runs, 2 * 4);
}

#[test]
fn every_queue_edit_in_the_switch_window_is_corrected() {
    use Msg::*;
    // mpv chose B at A's end before the edit's clear ran. The edit may
    // reach Coda before or after A's end-file; its replies (if it was sent
    // at all) may come before or after the events. The player corrects
    // by loading the right track: B never plays for the player, and a
    // removed B is replaced as soon as the load lands.
    let mut runs = 0;
    for (edit, next) in edit_cases() {
        for b_loaded in [false, true] {
            let setup = playing_with_next(&["A", "B", "C"]);
            let mut msgs = vec![
                edit,
                Clear(1),
                AppendOk(1, 3),
                End(1, "eof"),
                Start(2),
                Check(0, false),
            ];
            // The edit reaches the player before B's start does (after it,
            // it is an edit of what follows B).
            let mut before = vec![(0, 1), (1, 2), (3, 4), (3, 5), (0, 4)];
            if b_loaded {
                msgs.push(Loaded);
                before.push((4, 6));
            }
            // The correction replaces B.
            let corrected = group(
                &[LoadfileOk(1, 4), End(2, "stop"), Start(4), Loaded],
                &[(0, 1), (1, 2), (2, 3)],
            );
            runs += exhaust(
                &setup,
                &[group(&msgs, &before), corrected],
                &|sim, label| {
                    assert_eq!(sim.gapless, 0, "{label}");
                    assert_eq!(sim.corrections, 1, "{label}");
                    assert_eq!(sim.advances, 0, "{label}");
                    assert_eq!(sim.loads.len(), 2, "{label}");
                    assert_eq!(sim.sent_paths()[1], path(next), "{label}");
                    assert_eq!(sim.current_title(), next, "{label}");
                    assert_eq!(sim.started_current.len(), 2, "{label}");
                    assert_eq!(sim.heard_title(), next, "{label}");
                    assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
                    assert_eq!(sim.stops_sent(), 0, "{label}");
                    assert!(sim.messages.is_empty(), "{label}: {:?}", sim.messages);
                },
            );
        }
    }
    assert_eq!(runs, 2 * (35 + 81));
}

#[test]
fn every_queue_edit_that_left_mpv_idle_loads_the_new_next() {
    use Msg::*;
    // The clear ran before A ended (its reply comes first), the append
    // after: mpv had nothing to go on to. Its append reply may still come
    // before A's end-file.
    let mut runs = 0;
    for (edit, next) in edit_cases() {
        let setup = playing_with_next(&["A", "B", "C"]);
        let idle = group(
            &[
                edit,
                Clear(1),
                End(1, "eof"),
                AppendOk(1, 3),
                Check(0, true),
            ],
            &[(0, 1), (1, 2), (1, 3), (2, 4)],
        );
        let loaded = group(&[LoadfileOk(1, 4), Start(4), Loaded], &[(1, 2)]);
        runs += exhaust(&setup, &[idle, loaded], &|sim, label| {
            assert_eq!(sim.advances, 1, "{label}");
            assert_eq!(sim.gapless, 0, "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.current_title(), next, "{label}");
            assert_eq!(sim.heard_title(), next, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
        });
    }
    assert_eq!(runs, 2 * 3 * 3);
}

#[test]
fn removing_the_last_next_entry_in_the_switch_window_stops_it() {
    use Msg::*;
    // [A, B]: the user removes B while mpv is already starting it.
    let setup = playing_with_next(&["A", "B"]);
    let msgs = [
        RemoveNext,
        Clear(1),
        End(1, "eof"),
        Start(2),
        Check(0, false),
    ];
    let runs = exhaust(
        &setup,
        &[group(&msgs, &[(0, 1), (2, 3), (2, 4), (0, 3)])],
        &|sim, label| {
            assert_eq!(sim.corrections, 1, "{label}");
            assert_eq!(sim.loads.len(), 1, "{label}");
            assert_eq!(sim.stops_sent(), 1, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
            assert_eq!(sim.player.queue().len(), 1, "{label}");
            assert_eq!(sim.started_current.len(), 1, "{label}");
        },
    );
    assert_eq!(runs, 16);
}

#[test]
fn every_user_load_racing_a_gapless_advance_ends_on_the_requested_track() {
    use Msg::*;
    let mut runs = 0;
    // mpv went on to B by itself before the user's load replaced it.
    for b_loaded in [false, true] {
        let setup = playing_with_next(&["A", "B", "C", "D"]);
        let mut msgs = vec![
            Play(2),
            LoadfileOk(1, 3),
            End(1, "eof"),
            Start(2),
            End(2, "stop"),
            Start(3),
            Loaded,
            Check(0, false),
        ];
        let mut before = vec![(0, 1), (2, 3), (3, 4), (1, 4), (4, 5), (5, 6), (2, 7)];
        if b_loaded {
            msgs.push(Loaded);
            before.extend([(3, 8), (8, 4)]);
        }
        let settle = group(&[Settle], &[]);
        runs += exhaust(&setup, &[group(&msgs, &before), settle], &|sim, label| {
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.current_index(), Some(2), "{label}");
            assert_eq!(sim.heard_title(), "C", "{label}");
            assert_eq!(sim.advances, 0, "{label}");
            assert_eq!(sim.corrections, 0, "{label}");
            assert_eq!(sim.started_current.len(), 2, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.stops_sent(), 0, "{label}");
            // What follows C reaches mpv, also when C was loaded while
            // the end of A was still being settled.
            assert_eq!(sim.appended_paths().last(), Some(&path("D")), "{label}");
        });
    }
    // The user's load replaced A before it ended.
    let setup = playing_with_next(&["A", "B", "C", "D"]);
    let msgs = [Play(2), LoadfileOk(1, 3), End(1, "stop"), Start(3), Loaded];
    runs += exhaust(
        &setup,
        &[
            group(&msgs, &[(0, 1), (1, 2), (2, 3), (3, 4)]),
            group(&[Settle], &[]),
        ],
        &|sim, label| {
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.current_index(), Some(2), "{label}");
            assert_eq!(sim.gapless + sim.advances + sim.corrections, 0, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.appended_paths().last(), Some(&path("D")), "{label}");
        },
    );
    assert_eq!(runs, 38 + 75 + 1);
}

#[test]
fn every_stop_racing_a_gapless_advance_stays_stopped() {
    use Msg::*;
    let mut runs = 0;
    for b_loaded in [false, true] {
        let setup = playing_with_next(&["A", "B"]);
        // mpv went on to B before the stop ran.
        let mut msgs = vec![
            ClearQueue,
            End(1, "eof"),
            Start(2),
            End(2, "stop"),
            Check(0, false),
        ];
        let mut before = vec![(1, 2), (2, 3), (0, 3), (1, 4)];
        if b_loaded {
            msgs.push(Loaded);
            before.extend([(2, 5), (5, 3)]);
        }
        runs += exhaust(&setup, &[group(&msgs, &before)], &|sim, label| {
            assert_eq!(sim.loads.len(), 1, "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
            assert_eq!(sim.stops_sent(), 1, "{label}");
            assert_eq!(sim.started_current.len(), 1, "{label}");
            assert!(sim.player.queue().is_empty(), "{label}");
            assert!(sim.player.playing_entry().is_none(), "{label}");
        });
    }
    // The stop ran before A ended.
    let setup = playing_with_next(&["A", "B"]);
    runs += exhaust(
        &setup,
        &[group(&[ClearQueue, End(1, "stop")], &[(0, 1)])],
        &|sim, label| {
            assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
            assert_eq!(sim.stops_sent(), 1, "{label}");
        },
    );
    assert_eq!(runs, 11 + 19 + 1);
}

#[test]
fn every_failing_next_track_is_reported_and_skipped() {
    use Msg::*;
    // B is a 404 stream or a file deleted after it was held: mpv reports it
    // only at the change, then stops. The player goes on to C.
    let setup = playing_with_next(&["A", "B", "C"]);
    let failed = group(
        &[End(1, "eof"), Start(2), End(2, "error"), Check(0, false)],
        &[(0, 1), (1, 2), (0, 3)],
    );
    let loaded = group(&[LoadfileOk(1, 4), Start(4), Loaded], &[(1, 2)]);
    let runs = exhaust(&setup, &[failed, loaded], &|sim, label| {
        assert_eq!(sim.gapless, 1, "{label}");
        assert_eq!(sim.advances, 1, "{label}: B's error");
        assert_eq!(sim.loads.len(), 2, "{label}");
        assert_eq!(sim.current_index(), Some(2), "{label}");
        assert_eq!(
            sim.messages,
            ["Could not play B: loading failed"],
            "{label}"
        );
        assert_eq!(sim.started_current.len(), 2, "{label}: A and C");
        assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
    });
    // At the end of the queue: stopped.
    let setup = playing_with_next(&["A", "B"]);
    let failed = group(
        &[End(1, "eof"), Start(2), End(2, "error"), Check(0, false)],
        &[(0, 1), (1, 2), (0, 3)],
    );
    let stopped = exhaust(&setup, &[failed], &|sim, label| {
        assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{label}");
        assert_eq!(sim.loads.len(), 1, "{label}");
        assert_eq!(sim.messages.len(), 1, "{label}");
    });
    assert_eq!(runs + stopped, 3 * 3 + 3);
}

#[test]
fn a_missing_next_file_is_never_held_and_reported_when_reached() {
    let mut sim = Sim::new(&["A", "B", "C"]);
    sim.missing.push(path("B"));
    sim.play(0);
    sim.complete_load(0, 1);
    assert!(sim.appended_paths().is_empty(), "the precheck failed");
    sim.position(2.0);
    sim.end_file(1, "eof");
    // A ends as before M8; B's load is rejected and C loaded.
    assert_eq!(sim.messages, ["Could not play B: file not found"]);
    assert_eq!(sim.sent_paths(), [path("A"), path("C")]);
    sim.assert_outcome(2, 3);
}

/// Plays the queue with every track ending after `position` seconds, mpv
/// going on to whatever it holds next (answering every request), until
/// nothing more happens. Returns the titles that were heard, in order.
fn play_through_gaplessly(sim: &mut Sim, position: f64) -> Vec<String> {
    let mut heard = Vec::new();
    let mut entry = 1;
    let (mut clears, mut appends, mut checks) = (0, 0, 0);
    sim.play(0);
    sim.loadfile_ok(0, entry);
    sim.start_file(entry);
    sim.file_loaded();
    let mut loads_seen = 1;
    for _ in 0..40 {
        if sim.player.current_load().is_none() {
            return heard;
        }
        heard.push(sim.current_title());
        sim.position(position);
        // Answer the lookahead's requests: mpv holds the next entry.
        let mut held = None;
        while clears < sim.requests(|c| *c == MpvCommand::PlaylistClear).len() {
            sim.clear_ok(clears);
            clears += 1;
        }
        while appends < sim.appended_paths().len() {
            held = Some(entry + 1 + appends as i64);
            sim.append_ok(appends, entry + 1 + appends as i64);
            appends += 1;
        }
        sim.end_file(entry, "eof");
        match held {
            Some(next) if sim.checks_sent() > checks => {
                checks += 1;
                entry = next;
                sim.start_file(entry);
                sim.file_loaded();
                sim.position(0.05);
            }
            _ => {
                // Ended without a successor: a load follows, if anything.
                if sim.loads.len() > loads_seen {
                    loads_seen = sim.loads.len();
                    entry += 100;
                    sim.loadfile_ok(sim.sent_paths().len() - 1, entry);
                    sim.start_file(entry);
                    sim.file_loaded();
                }
            }
        }
    }
    panic!("did not terminate: {heard:?}");
}

#[test]
fn instant_files_never_loop_when_advances_are_gapless() {
    for mode in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
        let mut sim = repeating(&["A", "B", "C"], mode);
        let heard = play_through_gaplessly(&mut sim, 0.2);
        assert_eq!(heard, ["A", "B", "C"], "{mode:?}");
        assert_eq!(sim.player.status(), PlaybackStatus::Stopped, "{mode:?}");
        assert_eq!(
            sim.messages.last().map(String::as_str) == Some(NO_PLAYABLE_TRACKS),
            mode == RepeatMode::All,
            "{mode:?}: {:?}",
            sim.messages
        );
        assert!(sim.gapless >= 2, "{mode:?}: {}", sim.gapless);
    }
}

#[test]
fn repeat_one_with_proven_playback_repeats_without_a_gap() {
    let mut sim = repeating(&["A", "B"], RepeatMode::One);
    sim.play(0);
    sim.complete_load(0, 1);
    // Not proven yet: the queue's next entry is held.
    assert_eq!(sim.appended_paths(), [path("B")]);
    sim.clear_ok(0);
    sim.append_ok(0, 2);
    sim.position(2.0);
    // Proven: the track itself is held next.
    assert_eq!(sim.appended_paths(), [path("B"), path("A")]);
    sim.clear_ok(1);
    sim.append_ok(1, 3);
    sim.end_file(1, "eof");
    sim.start_file(3);
    sim.file_loaded();
    sim.position(0.1);
    sim.check(0, false);
    assert_eq!(sim.gapless, 1);
    assert_eq!(sim.loads.len(), 1);
    assert_eq!(sim.current_index(), Some(0));
    assert_eq!(sim.started_current.len(), 2, "each repeat is its own load");
    assert_eq!(sim.started_stale, 0);
}

#[test]
fn several_set_next_changes_collapse_to_the_latest() {
    use Msg::*;
    // B is on its way to mpv; meanwhile the user edits three times. Only
    // the latest next entry follows, once B's requests are answered.
    let setup = || {
        let mut sim = Sim::new(&["A", "B", "C", "D"]);
        sim.play(0);
        sim.complete_load(0, 1);
        sim.play_next("X");
        sim.play_next("Y");
        sim.remove_next();
        sim
    };
    let runs = exhaust(
        &setup,
        &[group(&[Clear(0), AppendOk(0, 2)], &[(0, 1)])],
        &|sim, label| {
            assert_eq!(sim.appended_paths(), [path("B"), path("Y")], "{label}");
        },
    );
    assert_eq!(runs, 1);
    // Then mpv takes Y and goes on to it.
    let mut sim = setup();
    sim.clear_ok(0);
    sim.append_ok(0, 2);
    sim.clear_ok(1);
    sim.append_ok(1, 3);
    sim.end_file(1, "eof");
    sim.start_file(3);
    assert_eq!(sim.gapless, 1);
    assert_eq!(sim.current_title(), "Y");
}

#[test]
fn an_append_left_behind_an_accepted_advance_is_cleared() {
    // [A, B], A proven. `r` three times in the switch window: all (B next),
    // one (A next), off (B next again). mpv had moved on to the first copy
    // of B, which the player accepts (the same local entry), but the later
    // copies were appended behind it. Nothing follows B, so the backend
    // must clear them, or mpv would start one at B's end.
    let mut sim = playing_with_next(&["A", "B"])();
    sim.position(2.0);
    sim.player.cycle_repeat();
    sim.commands(Vec::new());
    sim.player.cycle_repeat();
    sim.commands(Vec::new());
    assert_eq!(sim.appended_paths(), [path("B"), path("A")]);
    sim.clear_ok(1);
    sim.append_ok(1, 3);
    sim.player.cycle_repeat();
    sim.commands(Vec::new());
    assert_eq!(sim.appended_paths(), [path("B"), path("A"), path("B")]);
    sim.end_file(1, "eof");
    sim.start_file(2);
    assert_eq!(
        (sim.gapless, sim.corrections),
        (1, 0),
        "the first copy is accepted"
    );
    let clears = |sim: &Sim| sim.requests(|c| *c == MpvCommand::PlaylistClear).len();
    assert_eq!(
        clears(&sim),
        3,
        "nothing more until the open replies are in"
    );
    sim.clear_ok(2);
    sim.append_ok(2, 4);
    assert_eq!(clears(&sim), 4, "the leftover copies are cleared");
    assert_eq!(sim.appended_paths().len(), 3, "and nothing appended");
    sim.clear_ok(3);
    sim.file_loaded();
    sim.position(0.1);
    sim.position(2.0);
    assert_eq!(sim.heard_title(), "B");
    // B ends; mpv holds nothing after it.
    sim.end_file(2, "eof");
    sim.check(1, true);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert_eq!(sim.stops_sent(), 0, "B was not cut");
    assert_eq!(sim.corrections, 0);
    assert_eq!(sim.loads.len(), 1);
}
