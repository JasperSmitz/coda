//! Event-ordering safety (owner requirement, brief section 4).
//!
//! Drives the pure mpv state machine and the pure player together through
//! scripted mpv message sequences and proves that entry ids and
//! asynchronous events never advance the queue incorrectly. Every scenario
//! asserts the final queue position and the exact number of `Load`
//! commands the player emitted.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use coda_player::model::{Track, TrackMetadata};
use coda_player::playback::mpv::protocol::MpvCommand;
use coda_player::playback::mpv::state::{MpvState, OutgoingRequest, Output};
use coda_player::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent};
use coda_player::player::{PlaybackStatus, Player, PlayerCommand};
use coda_player::queue::QueueEntryId;
use coda_player::resolve::resolve_command;
use serde_json::json;

/// One `loadfile` that reached mpv, with the request ids of its replies.
#[derive(Debug, Clone)]
struct SentLoad {
    path: String,
    loadfile_request: u64,
    entry_id_request: u64,
}

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
    /// `Started` events for loads that were not current (stale).
    started_stale: usize,
    messages: Vec<String>,
}

impl Sim {
    fn new(names: &[&str]) -> Self {
        let mut player = Player::new();
        let ids = names
            .iter()
            .map(|n| {
                player.enqueue(Track::local(
                    format!("/m/{n}.wav").into(),
                    TrackMetadata::default(),
                ))
            })
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
            started_stale: 0,
            messages: Vec::new(),
        };
        sim.process(startup);
        sim
    }

    /// Resolves player commands as the app does (local tracks only, no
    /// Navidrome client), applies them to mpv and feeds resulting events
    /// back, until quiescent.
    fn commands(&mut self, commands: Vec<PlayerCommand>) {
        // Messages produced by the intent that created `commands`.
        self.messages.extend(self.player.take_messages());
        let mut work: VecDeque<PlayerCommand> = commands.into();
        while let Some(command) = work.pop_front() {
            let command = match resolve_command(command, None) {
                Ok(command) => command,
                Err((load, reason)) => panic!("{load}: local load not resolved: {reason}"),
            };
            if let PlaybackCommand::Load { load, .. } = &command {
                self.loads.push(*load);
            }
            let out = self.mpv.handle_command(command);
            work.extend(self.process(out));
        }
    }

    /// Records requests and delivers events to the player; returns the
    /// player's follow-up commands.
    fn process(&mut self, out: Output) -> Vec<PlayerCommand> {
        self.sent.extend(out.requests);
        let mut follow_up = Vec::new();
        for event in out.events {
            match &event {
                PlaybackEvent::Started { load } => {
                    if self.player.current_load() == Some(*load) {
                        self.started_current.push(*load);
                    } else {
                        self.started_stale += 1;
                    }
                }
                PlaybackEvent::TrackEnded {
                    load,
                    reason: EndReason::Eof | EndReason::Error(_),
                } if self.player.current_load() == Some(*load) => self.advances += 1,
                _ => {}
            }
            follow_up.extend(self.player.handle_event(event));
        }
        self.messages.extend(self.player.take_messages());
        follow_up
    }

    fn mpv_message(&mut self, line: &str) {
        let out = self
            .mpv
            .handle_line(line, Instant::now())
            .expect("valid line");
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

    // ----- scripted mpv messages ----------------------------------------

    fn sent_loads(&self) -> Vec<SentLoad> {
        let mut loads = Vec::new();
        for (i, request) in self.sent.iter().enumerate() {
            if let MpvCommand::LoadFile { path } = &request.command {
                let entry = self.sent[i + 1..]
                    .iter()
                    .find(|r| matches!(r.command, MpvCommand::GetProperty { .. }))
                    .expect("entry id query follows loadfile");
                loads.push(SentLoad {
                    path: path.clone(),
                    loadfile_request: request.request_id,
                    entry_id_request: entry.request_id,
                });
            }
        }
        loads
    }

    fn sent_paths(&self) -> Vec<String> {
        self.sent_loads().into_iter().map(|l| l.path).collect()
    }

    fn sent_load(&self, n: usize) -> SentLoad {
        self.sent_loads()[n].clone()
    }

    fn loadfile_ok(&mut self, n: usize, entry: i64) {
        let id = self.sent_load(n).loadfile_request;
        self.mpv_message(
            &json!({"data": {"playlist_entry_id": entry}, "request_id": id, "error": "success"})
                .to_string(),
        );
    }

    fn loadfile_err(&mut self, n: usize) {
        let id = self.sent_load(n).loadfile_request;
        self.mpv_message(&json!({"request_id": id, "error": "invalid parameter"}).to_string());
    }

    fn entry_id(&mut self, n: usize, entry: i64) {
        let id = self.sent_load(n).entry_id_request;
        self.mpv_message(&json!({"data": entry, "request_id": id, "error": "success"}).to_string());
    }

    fn start_file(&mut self, entry: i64) {
        self.mpv_message(&json!({"event": "start-file", "playlist_entry_id": entry}).to_string());
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

    /// Completes load `n` normally as entry `entry`: replies, start-file
    /// and file-loaded.
    fn complete_load(&mut self, n: usize, entry: i64) {
        self.loadfile_ok(n, entry);
        self.entry_id(n, entry);
        self.start_file(entry);
        self.file_loaded();
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::LoadfileOk(n, entry) => self.loadfile_ok(n, entry),
            Msg::LoadfileErr(n) => self.loadfile_err(n),
            Msg::EntryId(n, entry) => self.entry_id(n, entry),
            Msg::Start(entry) => self.start_file(entry),
            Msg::Loaded => self.file_loaded(),
            Msg::End(entry, reason) => self.end_file(entry, reason),
        }
    }

    fn stops_sent(&self) -> usize {
        self.sent
            .iter()
            .filter(|r| r.command == MpvCommand::Stop)
            .count()
    }

    // ----- assertions ---------------------------------------------------

    fn current_index(&self) -> Option<usize> {
        let current = self.player.queue().current()?;
        self.ids.iter().position(|id| *id == current)
    }

    fn assert_outcome(&self, current: usize, loads: usize) {
        assert_eq!(self.current_index(), Some(current), "queue position");
        assert_eq!(self.loads.len(), loads, "number of Load commands");
    }
}

fn path(name: &str) -> String {
    PathBuf::from(format!("/m/{name}.wav"))
        .to_string_lossy()
        .into_owned()
}

/// One scripted mpv message. `Loaded` (`file-loaded`) carries no entry id;
/// mpv attributes it to the preceding `start-file`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Msg {
    LoadfileOk(usize, i64),
    LoadfileErr(usize),
    EntryId(usize, i64),
    Start(i64),
    Loaded,
    End(i64, &'static str),
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

    // mpv processes B: replies, then replaces A.
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "stop");
    sim.entry_id(1, 2);
    // Only now does D go out; C never reaches mpv.
    assert_eq!(sim.sent_paths(), [path("A"), path("B"), path("D")]);
    sim.start_file(2);
    sim.file_loaded();
    sim.loadfile_ok(2, 3);
    sim.end_file(2, "stop");
    sim.entry_id(2, 3);
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
    sim.loadfile_ok(0, 1);
    sim.start_file(1);
    sim.file_loaded();
    sim.entry_id(0, 1);
    assert_eq!(sim.sent_paths(), [path("A"), path("D")]);
    sim.loadfile_ok(1, 2);
    sim.entry_id(1, 2);
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
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "eof"); // A also finished at the same moment
    sim.entry_id(1, 2);
    sim.start_file(2);
    sim.file_loaded();
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 0);
}

// ----- replacement during playback -----------------------------------------

#[test]
fn replacement_old_end_file_never_advances() {
    // The old entry's end-file with reason stop or eof, before or after the
    // new entry-id reply.
    for old_reason in ["stop", "eof"] {
        for old_before_reply in [true, false] {
            let mut sim = Sim::new(&["A", "B", "C"]);
            sim.play(0);
            sim.complete_load(0, 1);
            sim.press_n();
            sim.loadfile_ok(1, 2);
            if old_before_reply {
                sim.end_file(1, old_reason);
                sim.entry_id(1, 2);
            } else {
                sim.entry_id(1, 2);
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
fn replacement_old_end_file_before_loadfile_reply() {
    let mut sim = Sim::new(&["A", "B", "C"]);
    sim.play(0);
    sim.complete_load(0, 1);
    sim.press_n();
    sim.end_file(1, "eof");
    sim.loadfile_ok(1, 2);
    sim.entry_id(1, 2);
    sim.start_file(2);
    sim.file_loaded();
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 0);
}

#[test]
fn started_waits_for_file_loaded() {
    let mut sim = Sim::new(&["A"]);
    sim.play(0);
    sim.loadfile_ok(0, 1);
    sim.entry_id(0, 1);
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
    let commands = sim.player.play_track_now(x);
    sim.commands(commands);
    sim.loadfile_ok(1, 2);
    sim.end_file(1, "eof");
    sim.entry_id(1, 2);
    sim.start_file(2);
    sim.file_loaded();
    let titles: Vec<_> = sim
        .player
        .queue()
        .entries()
        .iter()
        .map(|e| e.track.title.as_str())
        .collect();
    assert_eq!(titles, ["A", "X", "B"]);
    assert_eq!(sim.player.queue().current_entry().unwrap().track.title, "X");
    assert_eq!(sim.loads.len(), 2);
    assert_eq!(sim.advances, 0);
}

// ----- very short files ----------------------------------------------------

#[test]
fn very_short_file_events_before_entry_id_reply() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.loadfile_ok(0, 1);
    sim.start_file(1);
    sim.file_loaded();
    sim.end_file(1, "eof");
    assert_eq!(sim.loads.len(), 1, "buffered until the id is known");
    sim.entry_id(0, 1);
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
fn very_short_file_events_before_loadfile_reply() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.start_file(1);
    sim.file_loaded();
    sim.end_file(1, "eof");
    sim.loadfile_ok(0, 1);
    sim.entry_id(0, 1);
    sim.assert_outcome(1, 2);
    assert_eq!(sim.advances, 1);
}

#[test]
fn queue_of_very_short_files_plays_through_once() {
    let mut sim = Sim::new(&["A", "B", "C", "D"]);
    sim.play(0);
    for n in 0..4 {
        let entry = n as i64 + 1;
        sim.loadfile_ok(n, entry);
        sim.start_file(entry);
        sim.file_loaded();
        sim.end_file(entry, "eof");
        sim.entry_id(n, entry);
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
fn failed_load_end_file_error_before_entry_id_reply() {
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.loadfile_ok(0, 1);
    sim.start_file(1);
    sim.end_file(1, "error");
    sim.entry_id(0, 1);
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
    // The id reply would name whatever entry was there before; ignored.
    sim.entry_id(0, 99);
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
    sim.entry_id(0, 1);
    sim.assert_outcome(0, 1);
    assert_eq!(sim.player.status(), PlaybackStatus::Stopped);
    assert!(sim.sent.iter().any(|r| r.command == MpvCommand::Stop));
}

fn reject_next(sim: &mut Sim, reason: &str) {
    let commands = sim.player.play_next();
    let [PlayerCommand::Load { load, .. }] = commands.as_slice() else {
        panic!("expected Load");
    };
    sim.loads.push(*load);
    let out = sim.mpv.load_rejected(*load, reason);
    let follow_up = sim.process(out);
    sim.commands(follow_up);
}

#[test]
fn missing_file_mid_skip_does_not_leave_old_track_playing() {
    // A plays; the user skips to B whose file vanished (rejected by the
    // driver without contacting mpv). B is last, so the player stops mpv.
    let mut sim = Sim::new(&["A", "B"]);
    sim.play(0);
    sim.complete_load(0, 1);
    reject_next(&mut sim, "file not found");
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
fn directory_target_is_rejected_and_skipped() {
    let mut sim = Sim::new(&["A", "Dir", "C"]);
    sim.play(0);
    sim.complete_load(0, 1);
    reject_next(&mut sim, "not a regular file");
    assert_eq!(sim.messages, ["Could not play Dir: not a regular file"]);
    sim.assert_outcome(2, 3);
    assert_eq!(sim.sent_paths(), [path("A"), path("C")]);
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
    reject_next(&mut sim, "file not found");
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
    sim.entry_id(1, 2);
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

/// A is playing as entry 1; `n` loads `names[1]` (load index 1).
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
            let mut msgs = vec![
                LoadfileOk(1, 2),
                EntryId(1, 2),
                Start(2),
                End(2, new_end),
                End(1, old),
            ];
            let mut before = vec![(0, 1), (2, 3)];
            if loaded {
                msgs.push(Loaded);
                before.extend([(2, 5), (5, 3)]);
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
        let msgs = [
            LoadfileOk(1, 2),
            EntryId(1, 2),
            Start(2),
            Loaded,
            End(1, old),
        ];
        runs += exhaust(&setup, &[group(&msgs, &[(0, 1), (2, 3)])], &|sim, label| {
            assert_eq!(sim.advances, 0, "{label}");
            assert_eq!(sim.current_index(), Some(1), "{label}");
            assert_eq!(sim.loads.len(), 2, "{label}");
            assert_eq!(sim.started_current, [sim.loads[0], sim.loads[1]], "{label}");
            assert_eq!(sim.player.status(), PlaybackStatus::Playing, "{label}");
            assert_eq!(sim.stops_sent(), 0, "{label}");
        });
    }
    assert_eq!(runs, 2 * (60 + 60 + 30 + 30));
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
            &[
                LoadfileOk(1, 2),
                EntryId(1, 2),
                Start(2),
                Loaded,
                End(1, old),
            ],
            &[(0, 1), (2, 3)],
        );
        // D is sent once B's id is known.
        let second = group(
            &[
                LoadfileOk(2, 3),
                EntryId(2, 3),
                End(2, "stop"),
                Start(3),
                Loaded,
            ],
            &[(0, 1), (3, 4)],
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
    assert_eq!(runs, 2 * 30 * 30);
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
            let mut msgs = vec![
                LoadfileOk(1, 2),
                EntryId(1, 2),
                Start(2),
                End(2, "stop"),
                End(1, old),
            ];
            let mut before = vec![(0, 1), (2, 3)];
            if loaded {
                msgs.push(Loaded);
                before.extend([(2, 5), (5, 3)]);
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
    assert_eq!(runs, 2 * (30 + 60));
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
    // The failed reply's id query still names A's entry (1); C is sent
    // after it. A ends either by finishing (eof) early or by C replacing it.
    let mut runs = exhaust(
        &setup,
        &[
            group(&[LoadfileErr(1), EntryId(1, 1), End(1, "eof")], &[(0, 1)]),
            group(
                &[LoadfileOk(2, 2), EntryId(2, 2), Start(2), Loaded],
                &[(0, 1), (2, 3)],
            ),
        ],
        &check,
    );
    runs += exhaust(
        &setup,
        &[
            group(&[LoadfileErr(1), EntryId(1, 1)], &[(0, 1)]),
            group(
                &[
                    LoadfileOk(2, 2),
                    EntryId(2, 2),
                    End(1, "stop"),
                    Start(2),
                    Loaded,
                ],
                &[(0, 1), (3, 4)],
            ),
        ],
        &check,
    );
    assert_eq!(runs, 3 * 6 + 30);
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
                EntryId(1, 2),
                Start(2),
                End(2, "redirect"),
                Start(3),
                Loaded,
                End(1, old),
            ],
            &[(0, 1), (2, 3), (3, 4), (4, 5)],
        )
    };
    let mut runs = 0;
    for old in ["stop", "eof"] {
        // With a next entry: the redirect is an error and C replaces it.
        let setup = playing_then_next(&["A", "B", "C"]);
        let second = group(
            &[
                LoadfileOk(2, 5),
                EntryId(2, 5),
                End(3, "stop"),
                Start(5),
                Loaded,
            ],
            &[(0, 1), (3, 4)],
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
    assert_eq!(runs, 2 * (105 * 30 + 105));
}
