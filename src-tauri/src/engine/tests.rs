//! Engine core tests: fake playback events, no window, no mpv (except the
//! `#[ignore]`d lifecycle test, which needs mpv on `PATH`).

use super::*;
use coda_core::navidrome::Password;
use coda_core::playback::{EndReason, PlaybackTarget, VisualizerFrame};
use std::sync::Mutex;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    /// A music folder with `names` as (empty) files.
    fn new(names: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for name in names {
            std::fs::write(root.join(name), b"").unwrap();
        }
        Self { _dir: dir, root }
    }

    fn track(&self, name: &str) -> Track {
        Track::local(
            self.root.join(name),
            TrackMetadata {
                title: Some(name.to_uppercase()),
                duration: Some(Duration::from_secs(200)),
                ..Default::default()
            },
        )
    }

    fn local_ref(&self, name: &str) -> TrackRef {
        TrackRef {
            source: SourceDto::Local {
                path: self.root.join(name).to_string_lossy().into_owned(),
            },
            title: Some("ignored".into()),
            artist: None,
            album: None,
            album_artist: None,
            track_no: None,
            disc_no: None,
            duration_ms: None,
        }
    }
}

fn sources(music_dir: Option<PathBuf>, client: Option<Client>) -> Sources {
    Sources {
        music_dir,
        scrobble: client.is_some(),
        client,
        cover_art: true,
        problems: Vec::new(),
    }
}

fn restored(tracks: Vec<Track>, current: Option<usize>) -> Restored {
    Restored {
        session: Session {
            tracks,
            current,
            ..Session::default()
        },
        save_path: None,
        notice: None,
    }
}

fn engine() -> Engine {
    Engine::new(Arc::new(CoverService::new(None)))
}

fn client() -> Client {
    Client::new(
        url::Url::parse("http://127.0.0.1:9/").unwrap(),
        "user".into(),
        Password::new("secret".into()),
    )
    .unwrap()
}

fn loads(commands: &[PlaybackCommand]) -> Vec<LoadId> {
    commands
        .iter()
        .filter_map(|c| match c {
            PlaybackCommand::Load { load, .. } => Some(*load),
            _ => None,
        })
        .collect()
}

fn keys(engine: &mut Engine) -> Vec<String> {
    engine
        .full_snapshot()
        .queue
        .unwrap()
        .into_iter()
        .map(|i| i.key)
        .collect()
}

/// A started engine with the library holding `names` and the queue
/// restored from them.
fn started(fixture: &Fixture, names: &[&str], current: Option<usize>) -> (Engine, Instant) {
    let now = Instant::now();
    let mut engine = engine();
    let tracks: Vec<Track> = names.iter().map(|n| fixture.track(n)).collect();
    engine.start(
        sources(Some(fixture.root.clone()), None),
        None,
        restored(tracks.clone(), current),
        now,
    );
    engine.handle_scan_event(ScanEvent::Batch(tracks), now);
    (engine, now)
}

#[test]
fn restore_never_plays_and_sets_the_current_entry() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let now = Instant::now();
    let mut engine = engine();
    let commands = engine.start(
        sources(None, None),
        None,
        restored(
            vec![fixture.track("a.flac"), fixture.track("b.flac")],
            Some(1),
        ),
        now,
    );
    assert!(loads(&commands).is_empty(), "{commands:?}");
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.phase, Phase::Ready);
    assert_eq!(snapshot.status, StatusDto::Stopped);
    let queue = snapshot.queue.unwrap();
    assert_eq!(queue.len(), 2);
    assert_eq!(snapshot.current_key.as_deref(), Some(queue[1].key.as_str()));
    assert_eq!(queue[1].track.title, "B.FLAC");
    // Restored state counts as saved.
    assert!(engine.take_final_save().is_none());
}

#[test]
fn intents_are_ignored_until_started() {
    let mut engine = engine();
    let now = Instant::now();
    assert!(engine.handle_intent(Intent::TogglePause, now).is_empty());
    assert_eq!(engine.full_snapshot().phase, Phase::Starting);
    // The visualizer wish is kept and applied at start.
    assert!(
        engine
            .handle_intent(Intent::SetVisualizerVisible { visible: true }, now)
            .is_empty()
    );
    let commands = engine.start(sources(None, None), None, restored(vec![], None), now);
    assert!(commands.contains(&PlaybackCommand::SetVisualizer(true)));
}

#[test]
fn play_pause_seek_and_volume_go_through_the_player() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], Some(0));
    let commands = engine.handle_intent(Intent::TogglePause, now);
    let load = loads(&commands)[0];
    match &commands[0] {
        PlaybackCommand::Load { target, .. } => {
            assert_eq!(target, &PlaybackTarget::File(fixture.root.join("a.flac")))
        }
        other => panic!("{other:?}"),
    }
    // The gapless next is announced with it.
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, PlaybackCommand::SetNext { next: Some(_), .. }))
    );
    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    engine.handle_playback_event(PlaybackEvent::Duration(Some(Duration::from_secs(200))), now);
    engine.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(10)), now);
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.status, StatusDto::Playing);
    assert_eq!(snapshot.playing.unwrap().title, "A.FLAC");
    assert_eq!(snapshot.up_next_key, Some(keys(&mut engine)[1].clone()));

    assert_eq!(
        engine.handle_intent(Intent::SeekTo { seconds: 42.0 }, now),
        vec![PlaybackCommand::SeekAbsolute(42.0)]
    );
    assert_eq!(
        engine.handle_intent(Intent::SeekBy { seconds: -5.0 }, now),
        vec![PlaybackCommand::SeekRelative(-5.0)]
    );
    assert_eq!(
        engine.handle_intent(Intent::SetVolume { volume: 40 }, now),
        vec![PlaybackCommand::SetVolume(40)]
    );
    assert_eq!(engine.full_snapshot().volume, 40);
    assert_eq!(
        engine.handle_intent(Intent::TogglePause, now),
        vec![PlaybackCommand::SetPause(true)]
    );
    assert_eq!(engine.full_snapshot().status, StatusDto::Paused);
}

#[test]
fn queue_entries_are_played_and_removed_by_key() {
    let fixture = Fixture::new(&["a.flac", "b.flac", "c.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac", "c.flac"], None);
    let keys_before = keys(&mut engine);
    let commands = engine.handle_intent(
        Intent::PlayEntry {
            key: keys_before[2].clone(),
        },
        now,
    );
    assert_eq!(loads(&commands).len(), 1);
    assert_eq!(
        engine.full_snapshot().current_key,
        Some(keys_before[2].clone())
    );
    engine.handle_intent(
        Intent::RemoveEntry {
            key: keys_before[0].clone(),
        },
        now,
    );
    // Keys are stable across edits.
    assert_eq!(keys(&mut engine), keys_before[1..].to_vec());
    // Unknown keys do nothing.
    assert!(
        engine
            .handle_intent(Intent::PlayEntry { key: "q999".into() }, now)
            .is_empty()
    );
    let commands = engine.handle_intent(Intent::ClearQueue, now);
    assert!(commands.contains(&PlaybackCommand::Stop), "{commands:?}");
    assert!(keys(&mut engine).is_empty());
}

#[test]
fn play_tracks_resolves_local_refs_through_the_library() {
    let fixture = Fixture::new(&["a.flac", "b.flac", "c.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], None);
    // c.flac exists but was never scanned: skipped, start moves with it.
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![
                fixture.local_ref("c.flac"),
                fixture.local_ref("a.flac"),
                fixture.local_ref("b.flac"),
            ],
            start: Some(2),
            shuffle: false,
        },
        now,
    );
    assert_eq!(loads(&commands).len(), 1);
    let snapshot = engine.full_snapshot();
    let queue = snapshot.queue.unwrap();
    assert_eq!(queue.len(), 2);
    // Metadata comes from the scan, not from the ref.
    assert_eq!(queue[0].track.title, "A.FLAC");
    assert_eq!(snapshot.current_key.as_deref(), Some(queue[1].key.as_str()));
    assert!(
        snapshot
            .notices
            .iter()
            .any(|n| n.text == "1 track not in the library; skipped"),
        "{:?}",
        snapshot.notices
    );
}

#[test]
fn play_tracks_shuffled_starts_with_the_chosen_track() {
    let names: Vec<String> = (0..20).map(|i| format!("{i}.flac")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let fixture = Fixture::new(&names);
    let (mut engine, now) = started(&fixture, &names, None);
    engine.handle_intent(
        Intent::PlayTracks {
            tracks: names.iter().map(|n| fixture.local_ref(n)).collect(),
            start: Some(7),
            shuffle: true,
        },
        now,
    );
    let snapshot = engine.full_snapshot();
    let queue = snapshot.queue.unwrap();
    assert_eq!(queue.len(), 20);
    assert_eq!(queue[0].track.title, "7.FLAC");
    assert_eq!(snapshot.current_key.as_deref(), Some(queue[0].key.as_str()));
}

#[test]
fn enqueue_adds_at_the_end_or_next() {
    let fixture = Fixture::new(&["a.flac", "b.flac", "c.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac", "c.flac"], None);
    engine.handle_intent(Intent::ClearQueue, now);
    engine.handle_intent(
        Intent::Enqueue {
            tracks: vec![fixture.local_ref("a.flac"), fixture.local_ref("b.flac")],
            next: false,
        },
        now,
    );
    let first = keys(&mut engine)[0].clone();
    engine.handle_intent(Intent::PlayEntry { key: first }, now);
    engine.handle_intent(
        Intent::Enqueue {
            tracks: vec![fixture.local_ref("c.flac")],
            next: true,
        },
        now,
    );
    let snapshot = engine.full_snapshot();
    let titles: Vec<String> = snapshot
        .queue
        .unwrap()
        .into_iter()
        .map(|i| i.track.title)
        .collect();
    assert_eq!(titles, ["A.FLAC", "C.FLAC", "B.FLAC"]);
    assert!(
        snapshot
            .notices
            .iter()
            .any(|n| n.text == "Playing next: 1 track")
    );
}

#[test]
fn navidrome_refs_are_built_from_their_metadata_and_scrobbled() {
    let now = Instant::now();
    let mut engine = engine();
    engine.start(
        sources(None, Some(client())),
        None,
        restored(vec![], None),
        now,
    );
    assert_eq!(
        engine.full_snapshot().navidrome.state,
        ConnectionDto::Connecting
    );
    let song = TrackRef {
        source: SourceDto::Navidrome {
            song_id: "song-1".into(),
            cover_art: Some("al-1".into()),
        },
        title: Some("Remote".into()),
        artist: Some("Band".into()),
        album: None,
        album_artist: None,
        track_no: Some(1),
        disc_no: None,
        duration_ms: Some(120_000),
    };
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![song],
            start: None,
            shuffle: false,
        },
        now,
    );
    let load = loads(&commands)[0];
    // The signed URL stays inside the backend command; the DTO has none.
    let snapshot = engine.full_snapshot();
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("secret") && !json.contains("token") && !json.contains("127.0.0.1"));
    let playing = snapshot.playing.unwrap();
    assert_eq!(playing.title, "Remote");
    assert_eq!(playing.duration_ms, Some(120_000));
    assert!(playing.cover_url.is_some());

    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    let scrobbles = engine.take_scrobbles();
    assert_eq!(
        scrobbles,
        vec![ScrobbleRequest::now_playing("song-1".into())]
    );
    // Half of a 2-minute track, in steps a listener could hear.
    for second in 1..=61 {
        engine.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(second)), now);
    }
    let scrobbles = engine.take_scrobbles();
    assert_eq!(scrobbles.len(), 1, "{scrobbles:?}");
    assert!(scrobbles[0].submission);

    engine.handle_ping(Ok(()));
    assert_eq!(
        engine.full_snapshot().navidrome.state,
        ConnectionDto::Connected
    );
}

#[test]
fn missing_mpv_is_a_problem_not_a_crash() {
    let fixture = Fixture::new(&["a.flac"]);
    let now = Instant::now();
    let mut engine = engine();
    let problem = ProblemDto {
        kind: ProblemKind::MpvMissing,
        message: "mpv was not found".into(),
    };
    let commands = engine.start(
        sources(None, None),
        Some(problem.clone()),
        restored(vec![fixture.track("a.flac")], Some(0)),
        now,
    );
    assert!(commands.is_empty(), "{commands:?}");
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.status, StatusDto::Unavailable);
    assert_eq!(snapshot.problems, vec![problem]);
    assert!(snapshot.notices.is_empty(), "{:?}", snapshot.notices);
    // The queue is still there, and intents are refused politely.
    assert_eq!(snapshot.queue.unwrap().len(), 1);
    assert!(engine.handle_intent(Intent::TogglePause, now).is_empty());
}

#[test]
fn a_backend_exit_becomes_a_problem() {
    let fixture = Fixture::new(&["a.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac"], Some(0));
    engine.handle_playback_event(PlaybackEvent::BackendExited("mpv crashed".into()), now);
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.status, StatusDto::Unavailable);
    assert_eq!(snapshot.problems.len(), 1);
    assert_eq!(snapshot.problems[0].kind, ProblemKind::Playback);
    assert!(snapshot.problems[0].message.contains("mpv crashed"));
}

#[test]
fn visualizer_readings_flow_only_while_visible_and_for_the_audible_track() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], Some(0));
    let load = loads(&engine.handle_intent(Intent::TogglePause, now))[0];
    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    let frame = VisualizerFrame {
        levels: [0.5, 0.4],
        bands: [Some(0.1); 7],
    };
    let reading = |load| PlaybackEvent::Visualizer { load, frame };
    // Hidden: mpv was never asked, readings are dropped.
    engine.handle_playback_event(reading(load), now);
    assert!(engine.take_frames().is_empty());
    assert_eq!(
        engine.handle_intent(Intent::SetVisualizerVisible { visible: true }, now),
        vec![PlaybackCommand::SetVisualizer(true)]
    );
    // Asking twice sends nothing more.
    assert!(
        engine
            .handle_intent(Intent::SetVisualizerVisible { visible: true }, now)
            .is_empty()
    );
    engine.handle_playback_event(reading(load), now);
    engine.handle_playback_event(reading(LoadId(load.0 + 100)), now);
    let frames = engine.take_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].levels, [0.5, 0.4]);
    // Paused: none.
    engine.handle_intent(Intent::TogglePause, now);
    engine.handle_playback_event(reading(load), now);
    assert!(engine.take_frames().is_empty());
    assert_eq!(
        engine.handle_intent(Intent::SetVisualizerVisible { visible: false }, now),
        vec![PlaybackCommand::SetVisualizer(false)]
    );
    // Unavailable: a notice, and never asked again.
    engine.handle_playback_event(PlaybackEvent::VisualizerUnavailable, now);
    assert!(
        engine
            .handle_intent(Intent::SetVisualizerVisible { visible: true }, now)
            .is_empty()
    );
    assert!(!engine.full_snapshot().visualizer_available);
}

#[test]
fn sessions_are_saved_after_a_quiet_debounce() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], Some(0));
    assert!(
        engine
            .take_due_save(now + Duration::from_secs(10))
            .is_none()
    );
    engine.handle_intent(Intent::CycleRepeat, now);
    assert!(engine.take_due_save(now + Duration::from_secs(1)).is_none());
    let later = now + Duration::from_secs(1);
    engine.handle_intent(Intent::CycleRepeat, later);
    // The debounce restarts with every change.
    assert!(engine.take_due_save(now + SAVE_DEBOUNCE).is_none());
    let session = engine.take_due_save(later + SAVE_DEBOUNCE).unwrap();
    assert_eq!(session.repeat, RepeatMode::One);
    assert_eq!(session.tracks.len(), 2);
    assert_eq!(session.current, Some(0));
    assert!(engine.take_final_save().is_none());
    // A failed write is retried at exit.
    engine.save_failed();
    assert!(engine.take_final_save().is_some());
    assert!(engine.take_final_save().is_none());
}

#[test]
fn snapshot_events_are_coalesced_and_carry_the_queue_only_when_it_changed() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], Some(0));
    let first = engine.take_snapshot_event().unwrap();
    assert_eq!(first.queue.as_ref().map(Vec::len), Some(2));
    assert!(engine.take_snapshot_event().is_none());
    // A change outside the queue: an event without the queue.
    engine.handle_intent(Intent::CycleRepeat, now);
    let event = engine.take_snapshot_event().unwrap();
    assert_eq!(event.repeat, RepeatDto::All);
    assert!(event.queue.is_none());
    // Changing the current entry is not a queue change either.
    let key = keys(&mut engine)[1].clone();
    engine.take_snapshot_event();
    engine.handle_intent(Intent::PlayEntry { key: key.clone() }, now);
    let event = engine.take_snapshot_event().unwrap();
    assert_eq!(event.current_key, Some(key.clone()));
    assert!(event.queue.is_none());
    // Something that changes nothing visible: no event.
    engine.handle_intent(Intent::PlayEntry { key: "nope".into() }, now);
    assert!(engine.take_snapshot_event().is_none());
    engine.handle_intent(Intent::RemoveEntry { key }, now);
    let event = engine.take_snapshot_event().unwrap();
    assert_eq!(event.queue.map(|q| q.len()), Some(1));
    // The command resets it: the next event repeats the queue.
    engine.full_snapshot();
    assert!(engine.take_snapshot_event().unwrap().queue.is_some());
}

#[test]
fn progress_is_sent_when_it_changes_at_most_every_interval() {
    let fixture = Fixture::new(&["a.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac"], Some(0));
    assert_eq!(
        engine.take_progress(now),
        Some(ProgressDto {
            position_ms: None,
            duration_ms: None
        })
    );
    let load = loads(&engine.handle_intent(Intent::TogglePause, now))[0];
    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    engine.handle_playback_event(PlaybackEvent::Position(Duration::from_millis(500)), now);
    assert!(
        engine
            .take_progress(now + Duration::from_millis(50))
            .is_none()
    );
    let later = now + PROGRESS_INTERVAL;
    assert_eq!(engine.take_progress(later).unwrap().position_ms, Some(500));
    // Unchanged: nothing.
    assert!(
        engine
            .take_progress(later + Duration::from_secs(1))
            .is_none()
    );
}

#[test]
fn notices_expire_and_can_be_dismissed() {
    let fixture = Fixture::new(&["a.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac"], Some(0));
    engine.handle_intent(Intent::CycleRepeat, now);
    let notices = engine.full_snapshot().notices;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].text, "Repeat all");
    engine.handle_intent(Intent::DismissNotice { id: notices[0].id }, now);
    assert!(engine.full_snapshot().notices.is_empty());
    engine.handle_intent(Intent::CycleRepeat, now);
    engine.tick(now + NOTICE_TTL);
    assert!(engine.full_snapshot().notices.is_empty());
}

#[test]
fn scan_progress_and_failure_are_reported() {
    let fixture = Fixture::new(&["a.flac"]);
    let now = Instant::now();
    let mut engine = engine();
    engine.start(
        sources(Some(fixture.root.clone()), None),
        None,
        restored(vec![], None),
        now,
    );
    assert_eq!(engine.full_snapshot().scan.state, ScanStateDto::Scanning);
    engine.handle_scan_event(ScanEvent::Batch(vec![fixture.track("a.flac")]), now);
    assert_eq!(engine.full_snapshot().scan.tracks, 1);
    engine.scan_closed(now);
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.scan.state, ScanStateDto::Failed);
    assert!(snapshot.notices[0].text.starts_with("Library scan failed"));
}

#[test]
fn a_track_that_fails_moves_on_and_reports_it() {
    let fixture = Fixture::new(&["a.flac", "b.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac", "b.flac"], Some(0));
    let load = loads(&engine.handle_intent(Intent::TogglePause, now))[0];
    let commands = engine.handle_playback_event(
        PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Error("bad file".into()),
        },
        now,
    );
    assert_eq!(loads(&commands).len(), 1, "{commands:?}");
    assert!(!engine.full_snapshot().notices.is_empty());
}

// ----- the loop, with real mpv ------------------------------------------

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Snapshot>>>);

impl Sink for Recorder {
    fn snapshot(&self, snapshot: &Snapshot) {
        self.0.lock().unwrap().push(snapshot.clone());
    }
    fn progress(&self, _: &ProgressDto) {}
    fn visualizer(&self, _: &VisualizerDto) {}
}

fn process_exists(pid: u32) -> bool {
    // Signal 0: only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[tokio::test]
async fn the_loop_starts_degraded_and_answers_without_mpv() {
    let (tx, rx) = mpsc::channel(16);
    let recorder = Recorder::default();
    let state = tempfile::tempdir().unwrap();
    let save_path = state.path().join("session.json");
    let startup = {
        let save_path = save_path.clone();
        async move {
            Startup {
                sources: sources(None, None),
                playback: Err(ProblemDto {
                    kind: ProblemKind::MpvMissing,
                    message: "mpv was not found".into(),
                }),
                restored: Restored {
                    session: Session::default(),
                    save_path: Some(save_path),
                    notice: None,
                },
            }
        }
    };
    let task = tokio::spawn(run(
        rx,
        recorder.clone(),
        Arc::new(CoverService::new(None)),
        startup,
    ));
    let handle = EngineHandle { tx };
    let mut snapshot = handle.snapshot().await.unwrap();
    while snapshot.phase != Phase::Ready {
        tokio::time::sleep(TICK).await;
        snapshot = handle.snapshot().await.unwrap();
    }
    assert_eq!(snapshot.problems[0].kind, ProblemKind::MpvMissing);
    handle.intent(Intent::CycleRepeat).await.unwrap();
    handle.shutdown().await;
    task.await.unwrap();
    let phases: Vec<Phase> = recorder.0.lock().unwrap().iter().map(|s| s.phase).collect();
    assert_eq!(phases.first(), Some(&Phase::Starting));
    assert_eq!(phases.last(), Some(&Phase::ShuttingDown));
    // The repeat change was saved at exit.
    let (saved, _) = session::load(&save_path);
    assert_eq!(saved.repeat, RepeatMode::All);
    assert!(handle.intent(Intent::Next).await.is_err());
}

#[tokio::test]
#[ignore = "needs mpv on PATH"]
async fn shutdown_saves_and_leaves_no_mpv_process() {
    let state = tempfile::tempdir().unwrap();
    let save_path = state.path().join("session.json");
    let fixture = Fixture::new(&["a.flac"]);
    let playback = coda_core::playback::spawn_backend().await.unwrap();
    let pid = playback.pid().unwrap();
    assert!(process_exists(pid));
    let restored = Restored {
        session: Session {
            tracks: vec![fixture.track("a.flac")],
            current: Some(0),
            ..Session::default()
        },
        save_path: Some(save_path.clone()),
        notice: None,
    };
    let (tx, rx) = mpsc::channel(16);
    let task = tokio::spawn(run(
        rx,
        Recorder::default(),
        Arc::new(CoverService::new(None)),
        async move {
            Startup {
                sources: sources(None, None),
                playback: Ok(playback),
                restored,
            }
        },
    ));
    let handle = EngineHandle { tx };
    while handle.snapshot().await.unwrap().phase != Phase::Ready {
        tokio::time::sleep(TICK).await;
    }
    handle.intent(Intent::CycleRepeat).await.unwrap();
    handle.shutdown().await;
    task.await.unwrap();
    // Reaped: give the OS a moment to drop the zombie entry.
    for _ in 0..20 {
        if !process_exists(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!process_exists(pid), "mpv {pid} still running");
    let (saved, _) = session::load(&save_path);
    assert_eq!(saved.repeat, RepeatMode::All);
    assert_eq!(saved.tracks.len(), 1);
}

// ----- live settings ----------------------------------------------------

fn remote_ref(song: &str) -> TrackRef {
    TrackRef {
        source: SourceDto::Navidrome {
            song_id: song.into(),
            cover_art: Some(format!("al-{song}")),
        },
        title: Some(song.into()),
        artist: None,
        album: None,
        album_artist: None,
        track_no: None,
        disc_no: None,
        duration_ms: Some(120_000),
    }
}

#[test]
fn settings_are_refused_until_started() {
    let mut engine = engine();
    let result = engine.apply_settings(SettingsChange::CoverArt(false), Instant::now());
    assert!(result.is_err());
}

#[test]
fn a_new_client_replaces_the_old_without_stopping_playback() {
    let now = Instant::now();
    let mut engine = engine();
    engine.start(
        sources(None, Some(client())),
        None,
        restored(vec![], None),
        now,
    );
    engine.handle_ping(Ok(()));
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![remote_ref("s1"), remote_ref("s2")],
            start: None,
            shuffle: false,
        },
        now,
    );
    let load = loads(&commands)[0];
    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    assert_eq!(engine.take_scrobbles().len(), 1, "now playing");
    let old_cover = engine.full_snapshot().playing.unwrap().cover_url;

    let commands = engine
        .apply_settings(
            SettingsChange::Client {
                client: Some(client()),
                scrobble: true,
            },
            now,
        )
        .unwrap();
    // The track heard plays on: nothing is loaded again.
    assert!(loads(&commands).is_empty(), "{commands:?}");
    assert_eq!(engine.full_snapshot().status, StatusDto::Playing);
    let change = engine.take_client_change().unwrap();
    assert!(change.ping && change.scrobble && change.client.is_some());
    assert_eq!(
        engine.full_snapshot().navidrome.state,
        ConnectionDto::Connecting
    );
    // Remote covers get new URLs (the old server's cache is not reused).
    assert_ne!(engine.full_snapshot().playing.unwrap().cover_url, old_cover);
    // The listen of the old server is not reported to the new one.
    for second in 1..=61 {
        engine.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(second)), now);
    }
    assert!(engine.take_scrobbles().is_empty());
    // The next track is resolved with the new client and scrobbled.
    let commands = engine.handle_intent(Intent::Next, now);
    let load = loads(&commands)[0];
    engine.handle_playback_event(PlaybackEvent::Started { load }, now);
    assert_eq!(
        engine.take_scrobbles(),
        vec![ScrobbleRequest::now_playing("s2".into())]
    );

    // Disconnected: no client, no scrobbling, a queued remote track no
    // longer resolves.
    engine
        .apply_settings(
            SettingsChange::Client {
                client: None,
                scrobble: false,
            },
            now,
        )
        .unwrap();
    let change = engine.take_client_change().unwrap();
    assert!(change.client.is_none() && !change.scrobble);
    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.navidrome.state, ConnectionDto::NotConfigured);
    assert!(!engine.scrobbling());
}

#[test]
fn a_new_music_folder_is_scanned_and_clears_the_no_config_problem() {
    let now = Instant::now();
    let mut engine = engine();
    let mut no_config = sources(None, None);
    no_config.problems.push(ProblemDto {
        kind: ProblemKind::NoConfig,
        message: "none".into(),
    });
    engine.start(no_config, None, restored(vec![], None), now);
    assert_eq!(engine.full_snapshot().problems.len(), 1);
    let fixture = Fixture::new(&["a.flac"]);
    engine
        .apply_settings(SettingsChange::MusicDir(fixture.root.clone()), now)
        .unwrap();
    assert_eq!(engine.take_rescan(), Some(fixture.root.clone()));
    assert_eq!(engine.take_rescan(), None, "once");
    let snapshot = engine.full_snapshot();
    assert!(snapshot.problems.is_empty());
    assert_eq!(snapshot.scan.state, ScanStateDto::Scanning);
    engine.handle_scan_event(ScanEvent::Batch(vec![fixture.track("a.flac")]), now);
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![fixture.local_ref("a.flac")],
            start: None,
            shuffle: false,
        },
        now,
    );
    assert_eq!(loads(&commands).len(), 1, "played from the new library");
}

#[test]
fn cover_art_and_scrobbling_switch_live() {
    let fixture = Fixture::new(&["a.flac"]);
    let (mut engine, now) = started(&fixture, &["a.flac"], Some(0));
    engine.take_snapshot_event();
    assert!(
        engine.full_snapshot().queue.unwrap()[0]
            .track
            .cover_url
            .is_some()
    );
    engine.take_snapshot_event();
    engine
        .apply_settings(SettingsChange::CoverArt(false), now)
        .unwrap();
    // The queue is sent again, without cover URLs.
    let event = engine.take_snapshot_event().unwrap();
    assert!(event.queue.unwrap()[0].track.cover_url.is_none());

    // Scrobbling needs a client: switching it on without one does nothing.
    engine
        .apply_settings(SettingsChange::Scrobble(true), now)
        .unwrap();
    assert!(!engine.scrobbling());
    assert!(engine.take_client_change().is_none());
    assert!(engine.scrobble_setting());
}

#[test]
fn scrobbling_can_be_switched_off_and_on() {
    let now = Instant::now();
    let mut engine = engine();
    engine.start(
        sources(None, Some(client())),
        None,
        restored(vec![], None),
        now,
    );
    assert!(engine.scrobbling());
    engine
        .apply_settings(SettingsChange::Scrobble(false), now)
        .unwrap();
    assert!(!engine.scrobbling());
    let change = engine.take_client_change().unwrap();
    assert!(!change.scrobble && !change.ping);
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![remote_ref("s1")],
            start: None,
            shuffle: false,
        },
        now,
    );
    engine.handle_playback_event(
        PlaybackEvent::Started {
            load: loads(&commands)[0],
        },
        now,
    );
    assert!(engine.take_scrobbles().is_empty());
    engine
        .apply_settings(SettingsChange::Scrobble(true), now)
        .unwrap();
    assert!(engine.take_client_change().unwrap().scrobble);
    // The track already playing is not reported halfway; the next one is.
    for second in 1..=61 {
        engine.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(second)), now);
    }
    assert!(engine.take_scrobbles().is_empty());
}
