//! Integration tests against a real mpv (`#[ignore]`d; run with
//! `cargo test -- --ignored`). WAV fixtures are generated per test.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use coda_tui::model::{Track, TrackMetadata};
use coda_tui::navidrome;
use coda_tui::playback::mpv::{self, MpvOptions};
use coda_tui::playback::{
    EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackHandle, PlaybackTarget,
    ShutdownOutcome, VisualizerFrame,
};
use coda_tui::player::{PlaybackStatus, Player, PlayerCommand};
use coda_tui::resolve;
use tokio::time::timeout;

const STEP: Duration = Duration::from_secs(5);

async fn backend() -> PlaybackHandle {
    mpv::spawn(MpvOptions::default()).await.expect("mpv starts")
}

/// A backend whose playback is timed by mpv's own clock (`--ao=null`), for
/// tests that need a track to keep playing for as long as it lasts.
///
/// With a real device, mpv's `eof` can come seconds before the audio is
/// heard: on macOS with Bluetooth headphones, CoreAudio rejects the mono
/// fixtures' channel layout, mpv falls back to its `avfoundation` output,
/// and that output takes about 4 s of 8 kHz audio up front. A 3 s fixture
/// then ends 0.1 s after it starts, and a 10 s one after 6.4 s.
async fn timed_backend() -> PlaybackHandle {
    mpv::spawn(MpvOptions {
        null_audio: true,
        ..MpvOptions::default()
    })
    .await
    .expect("mpv starts")
}

async fn next_event(handle: &mut PlaybackHandle) -> PlaybackEvent {
    timeout(STEP, handle.recv())
        .await
        .expect("event within timeout")
        .expect("backend still running")
}

/// Collects events until every predicate has matched some event, in any
/// order. Returns everything seen.
async fn wait_for_all(
    handle: &mut PlaybackHandle,
    preds: &[&dyn Fn(&PlaybackEvent) -> bool],
) -> Vec<PlaybackEvent> {
    let mut seen = Vec::new();
    while !preds.iter().all(|p| seen.iter().any(p)) {
        seen.push(next_event(handle).await);
    }
    seen
}

/// Waits for an event matching `pred`, returning it and everything seen.
async fn wait_for(
    handle: &mut PlaybackHandle,
    pred: impl Fn(&PlaybackEvent) -> bool,
) -> (PlaybackEvent, Vec<PlaybackEvent>) {
    let mut seen = Vec::new();
    loop {
        let event = next_event(handle).await;
        if pred(&event) {
            return (event, seen);
        }
        seen.push(event);
    }
}

fn load(n: u64, path: &Path) -> PlaybackCommand {
    PlaybackCommand::Load {
        load: LoadId(n),
        target: PlaybackTarget::File(path.to_path_buf()),
    }
}

/// `true` if `pid` is a live (non-zombie) process.
fn pid_alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// The private socket directory of a running mpv, read from its argv.
fn ipc_dir(pid: u32) -> PathBuf {
    let output = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let args = String::from_utf8_lossy(&output.stdout);
    // Not split on whitespace: the temp path may contain spaces.
    let start =
        args.find("--input-ipc-server=").expect("ipc argument") + "--input-ipc-server=".len();
    let end = start
        + args[start..]
            .find("/mpv.sock")
            .expect("socket file name in ipc argument");
    let dir = PathBuf::from(&args[start..end]);
    assert!(
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("coda-"),
        "{dir:?}"
    );
    assert!(dir.exists());
    dir
}

async fn wait_until_gone(dir: &Path) {
    for _ in 0..200 {
        if !dir.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{dir:?} still exists");
}

async fn wait_until_dead(pid: u32) {
    for _ in 0..200 {
        if !pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("mpv (pid {pid}) is still alive");
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn silent_wav_starts_then_ends_with_eof() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("one-second.wav");
    common::write_wav(&wav, 1000);
    let mut handle = backend().await;
    handle.send(load(1, &wav));
    // Started and Duration may arrive in either order; keep everything.
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert!(
        seen.contains(&PlaybackEvent::Started { load: LoadId(1) }),
        "{seen:?}"
    );
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Eof
        }
    );
    assert!(
        seen.iter().any(|e| matches!(e, PlaybackEvent::Duration(Some(d)) if d.abs_diff(Duration::from_secs(1)) < Duration::from_millis(50))),
        "{seen:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn volume_set_while_idle_is_accepted_and_echoed() {
    // A restored session sets the volume before anything is loaded.
    let mut handle = backend().await;
    handle.send(PlaybackCommand::SetVolume(40));
    let (_, seen) = wait_for(&mut handle, |e| *e == PlaybackEvent::Volume(40)).await;
    assert!(
        !seen.iter().any(|e| matches!(e, PlaybackEvent::Error(_))),
        "{seen:?}"
    );
    // It sticks for the next load.
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("short.wav");
    common::write_wav(&wav, 300);
    handle.send(load(1, &wav));
    let (_, seen) = wait_for(&mut handle, |e| {
        matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(1),
                ..
            }
        )
    })
    .await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, PlaybackEvent::Volume(v) if *v != 40)),
        "{seen:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn pause_volume_and_seek_are_reflected_in_events() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("long.wav");
    common::write_wav(&wav, 4000);
    // The track must still be playing when the seeks arrive.
    let mut handle = timed_backend().await;
    handle.send(load(1, &wav));
    // `Started` is mpv's file-loaded, so seeking works straight away. The
    // duration may arrive on either side of it; both must be seen.
    wait_for_all(
        &mut handle,
        &[
            &|e| *e == PlaybackEvent::Started { load: LoadId(1) },
            &|e| *e == PlaybackEvent::Duration(Some(Duration::from_secs(4))),
        ],
    )
    .await;

    handle.send(PlaybackCommand::SetPause(true));
    let (_, mut seen) = wait_for(&mut handle, |e| *e == PlaybackEvent::Paused(true)).await;
    handle.send(PlaybackCommand::SetVolume(40));
    seen.extend(
        wait_for(&mut handle, |e| *e == PlaybackEvent::Volume(40))
            .await
            .1,
    );
    let before = seen
        .iter()
        .rev()
        .find_map(|e| match e {
            PlaybackEvent::Position(p) => Some(*p),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        before < Duration::from_secs(1),
        "paused near the start: {before:?}"
    );
    // Under `--ao=null` mpv settles about 0.2 s short of the target, so
    // accept 2.0 s for a seek to 2.5 s; it must still be a clear jump
    // forward from the paused position.
    handle.send(PlaybackCommand::SeekAbsolute(2.5));
    wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::Position(p)
            if *p >= Duration::from_millis(2000) && *p >= before + Duration::from_millis(1500))
    })
    .await;
    handle.send(PlaybackCommand::SeekRelative(-2.0));
    wait_for(
        &mut handle,
        |e| matches!(e, PlaybackEvent::Position(p) if *p <= Duration::from_millis(700)),
    )
    .await;

    // A new load starts unpaused even though pause was on.
    handle.send(load(2, &wav));
    let (_, seen) = wait_for(&mut handle, |e| {
        *e == PlaybackEvent::Started { load: LoadId(2) }
    })
    .await;
    assert!(seen.contains(&PlaybackEvent::Paused(false)), "{seen:?}");
    handle.send(PlaybackCommand::Stop);
    let (ended, _) = wait_for(&mut handle, |e| {
        matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                ..
            }
        )
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(2),
            reason: EndReason::Stopped
        }
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn deleted_and_unplayable_files_end_with_error() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("gone.wav");
    common::write_wav(&wav, 100);
    std::fs::remove_file(&wav).unwrap();
    let junk = dir.path().join("junk.mp3");
    std::fs::write(&junk, b"definitely not audio").unwrap();

    let mut handle = backend().await;
    handle.send(load(1, &wav));
    assert_eq!(
        next_event(&mut handle).await,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Error("file not found".into())
        }
    );
    handle.send(load(2, &junk));
    let (ended, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert!(
        matches!(
            &ended,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                reason: EndReason::Error(_)
            }
        ),
        "{ended:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn hostile_file_names_play() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("-x \"q\" back\\slash\nnew line é 日本.wav");
    common::write_wav(&wav, 100);
    let mut handle = backend().await;
    handle.send(load(1, &wav));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert!(
        seen.contains(&PlaybackEvent::Started { load: LoadId(1) }),
        "{seen:?}"
    );
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Eof
        }
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn shutdown_exits_the_child() {
    let handle = backend().await;
    let pid = handle.pid().expect("pid");
    assert!(pid_alive(pid));
    let dir = ipc_dir(pid);
    assert_eq!(handle.shutdown(STEP).await, ShutdownOutcome::Clean);
    assert!(!pid_alive(pid), "mpv still running after shutdown");
    assert!(!dir.exists(), "socket dir not removed");
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn dropping_the_handle_kills_the_child() {
    let handle = backend().await;
    let pid = handle.pid().expect("pid");
    let dir = ipc_dir(pid);
    drop(handle);
    wait_until_dead(pid).await;
    wait_until_gone(&dir).await;
}

/// Dropping the runtime cancels the driver task without any graceful
/// shutdown: `kill_on_drop` must kill mpv and the socket dir must go.
#[test]
#[ignore = "needs mpv"]
fn dropping_the_runtime_kills_the_child_via_kill_on_drop() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let handle = runtime.block_on(backend());
    let pid = handle.pid().expect("pid");
    let dir = ipc_dir(pid);
    // The handle stays alive, so the command channel never closes: the
    // only cleanup left is dropping the cancelled task.
    drop(runtime);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while pid_alive(pid) {
        assert!(std::time::Instant::now() < deadline, "mpv survived");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!dir.exists(), "socket dir not removed");
    drop(handle);
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn shutdown_is_graceful_even_when_nobody_drains_events() {
    let mut handle = backend().await;
    let pid = handle.pid().unwrap();
    // Wait until startup settled, then flood the driver with commands whose
    // replies produce far more events than the channel holds, never
    // reading them.
    wait_for(&mut handle, |e| matches!(e, PlaybackEvent::Volume(_))).await;
    for i in 0..2000u32 {
        handle.send(PlaybackCommand::SetVolume((i % 100) as u8));
        handle.send(PlaybackCommand::SetPause(i % 2 == 0));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    let outcome = handle.shutdown(Duration::from_secs(5)).await;
    let elapsed = started.elapsed();
    assert_eq!(outcome, ShutdownOutcome::Clean);
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    assert!(!pid_alive(pid));
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn playlist_disguised_as_audio_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    common::write_wav(&a, 300);
    common::write_wav(&b, 300);
    let evil = dir.path().join("evil.mp3");
    std::fs::write(
        &evil,
        format!("#EXTM3U\n{}\n{}\n", a.display(), b.display()),
    )
    .unwrap();
    let mut handle = backend().await;
    handle.send(load(1, &evil));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert!(
        matches!(
            &ended,
            PlaybackEvent::TrackEnded {
                load: LoadId(1),
                reason: EndReason::Error(_)
            }
        ),
        "{ended:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, PlaybackEvent::Started { .. })),
        "{seen:?}"
    );
    // Nothing else plays afterwards.
    let quiet_until = tokio::time::Instant::now() + Duration::from_millis(800);
    while let Ok(Some(event)) = tokio::time::timeout_at(quiet_until, handle.recv()).await {
        assert!(
            !matches!(
                event,
                PlaybackEvent::Started { .. } | PlaybackEvent::TrackEnded { .. }
            ),
            "unrequested playback: {event:?}"
        );
        if let PlaybackEvent::Position(p) = event {
            assert!(p.is_zero(), "unrequested playback position {p:?}");
        }
    }
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn directory_target_is_rejected_before_mpv() {
    let dir = tempfile::tempdir().unwrap();
    common::write_wav(&dir.path().join("inside.wav"), 100);
    let mut handle = backend().await;
    handle.send(load(1, dir.path()));
    let (ended, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Error("not a regular file".into())
        }
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn killed_mpv_reports_backend_exited() {
    let mut handle = backend().await;
    let pid = handle.pid().unwrap();
    let dir = ipc_dir(pid);
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    let (event, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::BackendExited(_))
    })
    .await;
    assert!(matches!(event, PlaybackEvent::BackendExited(_)));
    // The task has ended: no more events, and commands are refused.
    assert!(timeout(STEP, handle.recv()).await.unwrap().is_none());
    assert!(!handle.send(PlaybackCommand::Stop));
    assert!(!dir.exists(), "socket dir not removed");
    assert!(!pid_alive(pid));

    // The player disables playback and does not restart anything.
    let mut player = Player::new();
    player.enqueue(Track::local("/m/a.wav".into(), TrackMetadata::default()));
    assert!(player.handle_event(event).is_empty());
    assert_eq!(player.status(), PlaybackStatus::Unavailable);
    assert!(player.toggle_pause().is_empty());
}

// ----- player + real mpv ---------------------------------------------------

struct Driven {
    /// Titles of tracks whose `Started` matched the player's current load.
    started: Vec<String>,
    /// `Load` commands sent.
    loads: usize,
    /// Gapless advances the player followed.
    gapless: usize,
}

/// Sends `initial`, then pumps events through the player until `done`.
/// Loads are resolved as the app does, signing remote tracks with
/// `navidrome`.
async fn drive(
    player: &mut Player,
    handle: &mut PlaybackHandle,
    initial: Vec<PlayerCommand>,
    navidrome: Option<&navidrome::Client>,
    done: impl Fn(&Player, &[String]) -> bool,
) -> Driven {
    let mut driven = Driven {
        started: Vec::new(),
        loads: 0,
        gapless: 0,
    };
    let send = |player: &mut Player,
                handle: &PlaybackHandle,
                commands: Vec<PlayerCommand>,
                driven: &mut Driven| {
        // The app's own dispatch: unresolvable loads advance the queue,
        // and the next track is announced.
        for command in resolve::dispatch(player, navidrome, commands) {
            if let PlaybackCommand::Load { .. } = &command {
                driven.loads += 1;
            }
            handle.send(command);
        }
    };
    send(player, handle, initial, &mut driven);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !done(player, &driven.started) {
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        let event = next_event(handle).await;
        let advanced_to = match &event {
            PlaybackEvent::Advanced { from, to } if player.current_load() == Some(*from) => {
                Some(*to)
            }
            _ => None,
        };
        let started = matches!(&event, PlaybackEvent::Started { load } if player.current_load() == Some(*load));
        let commands = player.handle_event(event);
        if started {
            let title = player.queue().current_entry().unwrap().track.title.clone();
            driven.started.push(title);
        }
        if advanced_to.is_some() && player.current_load() == advanced_to {
            driven.gapless += 1;
        }
        send(player, handle, commands, &mut driven);
    }
    driven
}

fn wav_queue(dir: &Path, names: &[&str], millis: u32) -> (Player, Vec<PathBuf>) {
    let mut player = Player::new();
    let mut paths = Vec::new();
    for name in names {
        let path = dir.join(format!("{name}.wav"));
        common::write_wav(&path, millis);
        player.enqueue(Track::local(path.clone(), TrackMetadata::default()));
        paths.push(path);
    }
    (player, paths)
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn queue_of_short_wavs_plays_through_in_order_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["t1", "t2", "t3", "t4", "t5"];
    let (mut player, _) = wav_queue(dir.path(), &names, 50);
    let mut handle = backend().await;
    let initial = player.toggle_pause();
    let driven = drive(&mut player, &mut handle, initial, None, |p, started| {
        !started.is_empty() && p.status() == PlaybackStatus::Stopped
    })
    .await;
    assert_eq!(driven.started, names);
    // Each track was loaded or reached without a gap, once.
    assert_eq!(driven.loads + driven.gapless, names.len());
    assert_eq!(player.queue().current_entry().unwrap().track.title, "t5");

    // Nothing further happens afterwards.
    let extra = timeout(Duration::from_millis(500), handle.recv()).await;
    if let Ok(Some(event)) = extra {
        assert!(player.handle_event(event).is_empty());
    }
    assert_eq!(player.status(), PlaybackStatus::Stopped);
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn rapid_loads_end_on_the_last_requested_track() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["r1", "r2", "r3", "r4", "r5"];
    let (mut player, _) = wav_queue(dir.path(), &names, 3000);
    // r4 must outlast the quiet period below, or its own (legitimate) end
    // advances the queue.
    let mut handle = timed_backend().await;
    // Space and three `n` presses before any event is processed.
    let mut initial = player.toggle_pause();
    initial.extend(player.play_next());
    initial.extend(player.play_next());
    initial.extend(player.play_next());
    let driven = drive(&mut player, &mut handle, initial, None, |p, _| {
        p.status() == PlaybackStatus::Playing
    })
    .await;
    assert_eq!(driven.started, ["r4"]);
    assert_eq!(driven.loads, 4);

    // Let stale events from the superseded loads drain: no advance.
    let quiet_until = tokio::time::Instant::now() + Duration::from_millis(700);
    while let Ok(Some(event)) = tokio::time::timeout_at(quiet_until, handle.recv()).await {
        assert!(player.handle_event(event).is_empty(), "unexpected advance");
    }
    assert_eq!(player.status(), PlaybackStatus::Playing);
    assert_eq!(player.queue().current_entry().unwrap().track.title, "r4");
    handle.shutdown(STEP).await;
}

/// A wiremock "Navidrome" serving `stream` for song `good` (a WAV of
/// `millis`) and 404 for anything else, plus a client signed for it.
async fn stream_server(millis: u32) -> (wiremock::MockServer, navidrome::Client) {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/music/rest/stream"))
        .and(query_param("id", "good"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(common::wav_bytes(millis), "audio/wav"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = navidrome::Client::new(
        navidrome::client::parse_base_url(&format!("{}/music", server.uri())).unwrap(),
        "u".into(),
        navidrome::Password::new("p".into()),
    )
    .unwrap();
    (server, client)
}

fn stream_load(n: u64, client: &navidrome::Client, song_id: &str) -> PlaybackCommand {
    PlaybackCommand::Load {
        load: LoadId(n),
        target: PlaybackTarget::Stream(client.stream_url(song_id).unwrap()),
    }
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn served_wav_stream_starts_then_ends_with_eof() {
    let (server, client) = stream_server(1000).await;
    let mut handle = backend().await;
    handle.send(stream_load(1, &client, "good"));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert!(
        seen.contains(&PlaybackEvent::Started { load: LoadId(1) }),
        "{seen:?}"
    );
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Eof
        }
    );
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.url.path() == "/music/rest/stream")
    );
    assert!(
        requests
            .iter()
            .all(|r| r.url.query_pairs().all(|(k, _)| k != "p"))
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn missing_stream_ends_with_error() {
    let (_server, client) = stream_server(1000).await;
    let mut handle = backend().await;
    handle.send(stream_load(1, &client, "missing"));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    let PlaybackEvent::TrackEnded {
        load: LoadId(1),
        reason: EndReason::Error(message),
    } = &ended
    else {
        panic!("expected an error end, got {ended:?} after {seen:?}");
    };
    assert!(!message.contains("t="), "{message}");
    assert!(
        !seen.contains(&PlaybackEvent::Started { load: LoadId(1) }),
        "{seen:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn mixed_local_and_remote_queue_plays_through_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, client) = stream_server(300).await;
    let (mut player, _) = wav_queue(dir.path(), &["local1"], 300);
    player.enqueue(Track::navidrome(
        "good".into(),
        TrackMetadata {
            title: Some("remote".into()),
            ..TrackMetadata::default()
        },
    ));
    player.enqueue(Track::navidrome(
        "missing".into(),
        TrackMetadata {
            title: Some("remote404".into()),
            ..TrackMetadata::default()
        },
    ));
    let path = dir.path().join("local2.wav");
    common::write_wav(&path, 300);
    player.enqueue(Track::local(path, TrackMetadata::default()));

    let mut handle = backend().await;
    let initial = player.toggle_pause();
    let driven = drive(
        &mut player,
        &mut handle,
        initial,
        Some(&client),
        |p, started| started.len() >= 3 && p.status() == PlaybackStatus::Stopped,
    )
    .await;
    assert_eq!(driven.started, ["local1", "remote", "local2"]);
    assert_eq!(driven.loads + driven.gapless, 4);
    let messages = player.take_messages();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("Could not play remote404")),
        "{messages:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
async fn nonexistent_program_is_not_found() {
    let err = mpv::spawn(MpvOptions {
        program: "/nonexistent/bin/mpv".into(),
        ..MpvOptions::default()
    })
    .await
    .err()
    .expect("spawn fails");
    assert!(err.is_not_found(), "{err:?}");
}

// ----- reply watchdog ------------------------------------------------------

#[tokio::test]
#[ignore = "needs mpv"]
async fn stopped_mpv_is_reported_as_not_responding_and_killed() {
    // SIGSTOP leaves mpv alive with its socket open, but it answers nothing.
    let reply_timeout = Duration::from_secs(1);
    let mut handle = mpv::spawn(MpvOptions {
        reply_timeout,
        ..MpvOptions::default()
    })
    .await
    .expect("mpv starts");
    let pid = handle.pid().unwrap();
    let dir = ipc_dir(pid);
    // Startup settled: its requests were answered.
    wait_for(&mut handle, |e| matches!(e, PlaybackEvent::Volume(_))).await;
    std::process::Command::new("kill")
        .args(["-STOP", &pid.to_string()])
        .status()
        .unwrap();
    let sent = tokio::time::Instant::now();
    handle.send(PlaybackCommand::SetPause(true));
    let (event, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::BackendExited(_))
    })
    .await;
    let elapsed = sent.elapsed();
    assert_eq!(
        event,
        PlaybackEvent::BackendExited("mpv stopped responding".into())
    );
    assert!(elapsed >= reply_timeout, "{elapsed:?}");
    assert!(elapsed < reply_timeout * 3, "{elapsed:?}");
    assert!(!pid_alive(pid), "hung mpv was not killed");
    assert!(!dir.exists(), "socket dir not removed");
    assert!(timeout(STEP, handle.recv()).await.unwrap().is_none());

    // The player treats it like any crashed backend.
    let mut player = Player::new();
    player.enqueue(Track::local("/m/a.wav".into(), TrackMetadata::default()));
    player.handle_event(event);
    assert_eq!(player.status(), PlaybackStatus::Unavailable);
    assert_eq!(player.unavailable_reason(), Some("mpv stopped responding"));
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn stalled_stream_does_not_trip_the_watchdog() {
    // A server that accepts connections and never sends a byte: mpv keeps
    // opening the stream, but still answers every request at once.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let client = navidrome::Client::new(
        navidrome::client::parse_base_url(&format!("http://{address}/music")).unwrap(),
        "u".into(),
        navidrome::Password::new("p".into()),
    )
    .unwrap();
    let reply_timeout = Duration::from_millis(1000);
    let mut handle = mpv::spawn(MpvOptions {
        reply_timeout,
        ..MpvOptions::default()
    })
    .await
    .expect("mpv starts");
    let pid = handle.pid().unwrap();
    handle.send(stream_load(1, &client, "slow"));
    // Keep requests flowing for several timeouts while the open hangs.
    let until = tokio::time::Instant::now() + reply_timeout * 4;
    let mut volume = 50;
    while tokio::time::Instant::now() < until {
        handle.send(PlaybackCommand::SetVolume(volume));
        volume = if volume == 50 { 60 } else { 50 };
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(200), handle.recv()).await
        {
            assert!(
                !matches!(
                    event,
                    PlaybackEvent::BackendExited(_)
                        | PlaybackEvent::TrackEnded { .. }
                        | PlaybackEvent::Started { .. }
                ),
                "{event:?}"
            );
        }
    }
    assert!(pid_alive(pid));
    handle.send(PlaybackCommand::Stop);
    let (ended, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Stopped
        }
    );
    assert_eq!(handle.shutdown(STEP).await, ShutdownOutcome::Clean);
    server.abort();
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn draining_the_audio_buffer_at_the_end_does_not_trip_the_watchdog() {
    // After the last entry's `end-file eof`, mpv blocks its core until the
    // buffered audio has played: here, 3 s of a 3 s file taken up front.
    // Requests sent meanwhile wait for that, which must stay within the
    // (default) reply timeout.
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("buffered.wav");
    common::write_wav(&wav, 3000);
    let options = MpvOptions {
        null_audio: true,
        null_audio_buffer: Some(3),
        ..MpvOptions::default()
    };
    assert_eq!(options.reply_timeout, Duration::from_secs(10));
    let mut handle = mpv::spawn(options).await.expect("mpv starts");
    handle.send(load(1, &wav));
    let (ended, _) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Eof
        }
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sent = tokio::time::Instant::now();
    handle.send(PlaybackCommand::SetVolume(50));
    handle.send(load(2, &wav));
    let (_, seen) = wait_for(&mut handle, |e| {
        *e == PlaybackEvent::Started { load: LoadId(2) }
    })
    .await;
    let waited = sent.elapsed();
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, PlaybackEvent::BackendExited(_))),
        "{seen:?}"
    );
    // The stall really happened (mpv answered only after draining).
    assert!(waited >= Duration::from_millis(1500), "{waited:?}");
    handle.shutdown(STEP).await;
}

// ----- visualizer -------------------------------------------------------

/// Band index (low to high) with the loudest level; hidden bands count as
/// silent.
fn loudest_band(frame: &VisualizerFrame) -> usize {
    let mut best = 0;
    for (i, band) in frame.bands.iter().enumerate() {
        if band.unwrap_or(0.0) > frame.bands[best].unwrap_or(0.0) {
            best = i;
        }
    }
    best
}

/// Collects `(seconds since start, frame)` of `load`'s visualizer events
/// until its track ends; returns them with the end reason.
async fn visualizer_readings(
    handle: &mut PlaybackHandle,
    load: LoadId,
    start: tokio::time::Instant,
) -> (Vec<(f64, VisualizerFrame)>, EndReason) {
    let mut readings = Vec::new();
    loop {
        match timeout(Duration::from_secs(15), handle.recv())
            .await
            .expect("event within timeout")
            .expect("backend still running")
        {
            PlaybackEvent::Visualizer { load: l, frame } => {
                assert_eq!(l, load);
                readings.push((start.elapsed().as_secs_f64(), frame));
            }
            PlaybackEvent::TrackEnded { load: l, reason } if l == load => {
                return (readings, reason);
            }
            PlaybackEvent::VisualizerUnavailable => panic!("visualizer unavailable"),
            _ => {}
        }
    }
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn visualizer_tones_peak_in_their_band_when_they_are_heard() {
    // 63 Hz, 400 Hz, 2.5 kHz and 6.3 kHz for 1.5 s each: bands 0, 2, 4, 5.
    const SEGMENT: f64 = 1.5;
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tones.wav");
    let tones = [(63.0, 1500), (400.0, 1500), (2500.0, 1500), (6300.0, 1500)];
    common::write_tones(&wav, 44100, 2, &tones, 0.25);
    let mut handle = timed_backend().await;
    handle.send(PlaybackCommand::SetVisualizer(true));
    handle.send(load(1, &wav));
    let (_, _) = wait_for(&mut handle, |e| {
        *e == PlaybackEvent::Started { load: LoadId(1) }
    })
    .await;
    let start = tokio::time::Instant::now();
    let (readings, reason) = visualizer_readings(&mut handle, LoadId(1), start).await;
    assert_eq!(reason, EndReason::Eof);
    // About 21 readings a second (2048-sample chunks at 44.1 kHz).
    let rate = readings.len() as f64 / (SEGMENT * 4.0);
    assert!((12.0..=31.0).contains(&rate), "{rate}/s");

    // Runs of the loudest band, ignoring flickers at the boundaries.
    let mut runs: Vec<(usize, usize, f64)> = Vec::new();
    for (at, frame) in &readings {
        let band = loudest_band(frame);
        match runs.last_mut() {
            Some((b, n, _)) if *b == band => *n += 1,
            _ => runs.push((band, 1, *at)),
        }
        for value in frame
            .levels
            .into_iter()
            .chain(frame.bands.into_iter().flatten())
        {
            assert!((0.0..=1.0).contains(&value), "{frame:?}");
        }
        assert!(
            frame.bands.iter().all(Option::is_some),
            "44.1 kHz: {frame:?}"
        );
    }
    let steady: Vec<(usize, f64)> = runs
        .iter()
        .filter(|(_, n, _)| *n >= 5)
        .map(|&(b, _, at)| (b, at))
        .collect();
    let bands: Vec<usize> = steady.iter().map(|&(b, _)| b).collect();
    assert_eq!(bands, [0, 2, 4, 5], "{runs:?}");
    // Each tone shows when it is heard, not when mpv analyses it (about
    // 0.4 s earlier under `--ao=null`). `start` is `file-loaded`, a little
    // before the audio starts.
    for (i, &(band, at)) in steady.iter().enumerate().skip(1) {
        let expected = SEGMENT * i as f64;
        let early = expected - at;
        eprintln!("band {band}: shown at {at:.3} s, tone starts at {expected:.1} s");
        assert!(
            (-0.35..0.15).contains(&early),
            "band {band} shown at {at:.3} s, tone at {expected} s: {runs:?}"
        );
    }
    // The level meter follows the signal: about -15 dBFS RMS per channel.
    let (_, middle) = readings[readings.len() / 2];
    assert!(
        middle.levels.iter().all(|l| (0.55..0.95).contains(l)),
        "{middle:?}"
    );
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn visualizer_graph_leaves_the_audio_bit_identical() {
    // Rendered to raw PCM with and without the graph (here passed at spawn
    // so it covers the whole file; Coda adds it while playing, which the
    // M6 spike showed to be lossless as well).
    let dir = tempfile::tempdir().unwrap();
    let tones = [(440.0, 700), (0.0, 100), (5000.0, 700)];
    let inputs = [
        ("mono", 1u16, 44100u32),
        ("stereo", 2, 48000),
        ("surround", 6, 44100),
    ];
    for (name, channels, rate) in inputs {
        let wav = dir.path().join(format!("{name}.wav"));
        common::write_tones(&wav, rate, channels, &tones, 0.4);
        let mut outputs = Vec::new();
        for with_graph in [false, true] {
            let out = dir.path().join(format!("{name}-{with_graph}.raw"));
            let mut extra_args = vec![
                "--ao=pcm".to_owned(),
                format!("--ao-pcm-file={}", out.display()),
                "--ao-pcm-waveheader=no".to_owned(),
            ];
            if with_graph {
                extra_args.push(format!("--af={}", mpv::visualizer::GRAPH));
            }
            let mut handle = mpv::spawn(MpvOptions {
                extra_args,
                ..MpvOptions::default()
            })
            .await
            .expect("mpv starts");
            handle.send(load(1, &wav));
            let (ended, _) = wait_for(&mut handle, |e| {
                matches!(e, PlaybackEvent::TrackEnded { .. })
            })
            .await;
            assert_eq!(
                ended,
                PlaybackEvent::TrackEnded {
                    load: LoadId(1),
                    reason: EndReason::Eof
                },
                "{name} with graph: {with_graph}"
            );
            assert_eq!(handle.shutdown(STEP).await, ShutdownOutcome::Clean);
            outputs.push(std::fs::read(&out).expect("pcm output"));
        }
        // Stereo out, 1.5 s: at least 4 bytes per frame.
        let frames = u64::from(rate) * 1500 / 1000;
        assert!(
            outputs[0].len() as u64 >= frames * 4,
            "{name}: {}",
            outputs[0].len()
        );
        assert!(outputs[0].iter().any(|&b| b != 0), "{name}: silent");
        assert_eq!(outputs[0].len(), outputs[1].len(), "{name}");
        assert!(
            outputs[0] == outputs[1],
            "{name}: output differs with the graph"
        );
    }
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn a_visualizer_graph_mpv_refuses_is_unavailable_and_playback_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    common::write_tones(&wav, 44100, 2, &[(440.0, 1500)], 0.3);
    let mut handle = mpv::spawn(MpvOptions {
        null_audio: true,
        visualizer_graph: Some(
            "@vizfmt:format=channels=stereo,@viz:lavfi=[nosuchfilter_coda_test]".into(),
        ),
        ..MpvOptions::default()
    })
    .await
    .expect("mpv starts");
    handle.send(PlaybackCommand::SetVisualizer(true));
    handle.send(load(1, &wav));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(1),
            reason: EndReason::Eof
        },
        "{seen:?}"
    );
    let unavailable = seen
        .iter()
        .filter(|e| **e == PlaybackEvent::VisualizerUnavailable)
        .count();
    assert_eq!(unavailable, 1, "{seen:?}");
    assert!(
        seen.iter()
            .any(|e| matches!(e, PlaybackEvent::Position(p) if *p >= Duration::from_millis(750))),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|e| matches!(
            e,
            PlaybackEvent::Visualizer { .. } | PlaybackEvent::Error(_)
        )),
        "{seen:?}"
    );
    // The next track plays too, and nothing is retried or reported again.
    handle.send(PlaybackCommand::SetVisualizer(false));
    handle.send(PlaybackCommand::SetVisualizer(true));
    handle.send(load(2, &wav));
    let (ended, seen) = wait_for(&mut handle, |e| {
        matches!(e, PlaybackEvent::TrackEnded { .. })
    })
    .await;
    assert_eq!(
        ended,
        PlaybackEvent::TrackEnded {
            load: LoadId(2),
            reason: EndReason::Eof
        },
        "{seen:?}"
    );
    assert!(seen.contains(&PlaybackEvent::Started { load: LoadId(2) }));
    assert!(
        !seen.iter().any(|e| matches!(
            e,
            PlaybackEvent::VisualizerUnavailable | PlaybackEvent::Visualizer { .. }
        )),
        "{seen:?}"
    );
    handle.shutdown(STEP).await;
}

/// Visualizer events among everything received for `period`.
async fn visualizer_events_during(handle: &mut PlaybackHandle, period: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + period;
    let mut count = 0;
    while let Ok(event) = tokio::time::timeout_at(deadline, handle.recv()).await {
        let event = event.expect("backend still running");
        assert!(
            !matches!(
                event,
                PlaybackEvent::TrackEnded { .. } | PlaybackEvent::BackendExited(_)
            ),
            "{event:?}"
        );
        if matches!(event, PlaybackEvent::Visualizer { .. }) {
            count += 1;
        }
    }
    count
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn no_visualizer_readings_while_hidden_or_paused() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    common::write_tones(&wav, 44100, 2, &[(1000.0, 8000)], 0.3);
    let mut handle = timed_backend().await;
    handle.send(load(1, &wav));
    wait_for(&mut handle, |e| {
        *e == PlaybackEvent::Started { load: LoadId(1) }
    })
    .await;
    // Off by default.
    assert_eq!(
        visualizer_events_during(&mut handle, Duration::from_millis(700)).await,
        0
    );
    // Shown mid-track: added then and there. The first readings are shown
    // only once the audio they describe is heard (about 0.4 s later here).
    handle.send(PlaybackCommand::SetVisualizer(true));
    let shown = visualizer_events_during(&mut handle, Duration::from_millis(1200)).await;
    assert!(shown >= 5, "{shown}");
    // Paused: none (after what was already on its way).
    handle.send(PlaybackCommand::SetPause(true));
    wait_for(&mut handle, |e| *e == PlaybackEvent::Paused(true)).await;
    visualizer_events_during(&mut handle, Duration::from_millis(100)).await;
    assert_eq!(
        visualizer_events_during(&mut handle, Duration::from_millis(700)).await,
        0
    );
    handle.send(PlaybackCommand::SetPause(false));
    let resumed = visualizer_events_during(&mut handle, Duration::from_millis(1000)).await;
    assert!(resumed >= 5, "{resumed}");
    // Hidden: none, though the graph stays until the track changes.
    handle.send(PlaybackCommand::SetVisualizer(false));
    visualizer_events_during(&mut handle, Duration::from_millis(100)).await;
    assert_eq!(
        visualizer_events_during(&mut handle, Duration::from_millis(700)).await,
        0
    );
    // Shown again on the same track: at once, without adding it twice.
    handle.send(PlaybackCommand::SetVisualizer(true));
    let again = visualizer_events_during(&mut handle, Duration::from_millis(1000)).await;
    assert!(again >= 5, "{again}");
    handle.shutdown(STEP).await;
}

/// Serves `body` as audio/wav to every connection, without Range support
/// and throttled to `bytes_per_sec`: a stream mpv cannot seek in beyond
/// what it has received.
async fn throttled_server(
    body: Vec<u8>,
    bytes_per_sec: usize,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = std::sync::Arc::new(body);
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let _ = socket.read(&mut request).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if socket.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for chunk in body.chunks(bytes_per_sec / 20) {
                    if socket.write_all(chunk).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
        }
    });
    (format!("http://{address}/music"), task)
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn a_seek_mpv_cannot_perform_does_not_freeze_the_visualizer() {
    // mpv answers `success` to a seek past what a non-seekable stream has
    // delivered, and sends neither `seek` nor `playback-restart`.
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    common::write_tones(&wav, 44100, 2, &[(1000.0, 30_000)], 0.3);
    // Twice real time: playable, never far ahead.
    let (base, server) = throttled_server(std::fs::read(&wav).unwrap(), 2 * 44100 * 4).await;
    let client = navidrome::Client::new(
        navidrome::client::parse_base_url(&base).unwrap(),
        "u".into(),
        navidrome::Password::new("p".into()),
    )
    .unwrap();
    let mut handle = timed_backend().await;
    handle.send(PlaybackCommand::SetVisualizer(true));
    handle.send(stream_load(1, &client, "song"));
    wait_for(&mut handle, |e| {
        *e == PlaybackEvent::Started { load: LoadId(1) }
    })
    .await;
    let before = visualizer_events_during(&mut handle, Duration::from_millis(1500)).await;
    assert!(before >= 5, "{before}");
    handle.send(PlaybackCommand::SeekRelative(20.0));
    visualizer_events_during(&mut handle, Duration::from_millis(200)).await;
    let after = visualizer_events_during(&mut handle, Duration::from_millis(1500)).await;
    assert!(after >= 5, "readings stopped after the seek: {after}");
    handle.shutdown(STEP).await;
    server.abort();
}

// ----- gapless playback (M8) ---------------------------------------------------

fn set_next(after: u64, next: u64, path: &Path) -> PlaybackCommand {
    PlaybackCommand::SetNext {
        after: LoadId(after),
        next: Some((LoadId(next), PlaybackTarget::File(path.to_path_buf()))),
    }
}

/// Events until `pred` matches (included), each with the time it arrived
/// since `start`.
async fn timed_until(
    handle: &mut PlaybackHandle,
    start: tokio::time::Instant,
    pred: impl Fn(&PlaybackEvent) -> bool,
) -> Vec<(f64, PlaybackEvent)> {
    let mut seen = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(15), handle.recv())
            .await
            .expect("event within timeout")
            .expect("backend still running");
        let done = pred(&event);
        seen.push((start.elapsed().as_secs_f64(), event));
        if done {
            return seen;
        }
    }
}

fn at(seen: &[(f64, PlaybackEvent)], event: &PlaybackEvent) -> f64 {
    seen.iter()
        .find(|(_, e)| e == event)
        .unwrap_or_else(|| panic!("{event:?} not in {seen:?}"))
        .0
}

const ADVANCED: PlaybackEvent = PlaybackEvent::Advanced {
    from: LoadId(1),
    to: LoadId(2),
};

#[tokio::test]
#[ignore = "needs mpv"]
async fn consecutive_tracks_render_bit_identically_across_a_gapless_join() {
    // Seeded noise cut at an odd sample into A (20 s) and B (1 s): played
    // back to back through the backend, the halves must give back the
    // uncut signal, byte for byte.
    const RATE: usize = 44100;
    let dir = tempfile::tempdir().unwrap();
    let whole = common::noise((RATE * 21 + 123) * 2, 7);
    let cut = (RATE * 20 + 123) * 2;
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    common::write_samples(&a, RATE as u32, 2, &whole[..cut]);
    common::write_samples(&b, RATE as u32, 2, &whole[cut..]);
    let reference: Vec<u8> = whole.iter().flat_map(|s| s.to_le_bytes()).collect();
    // `--ao=pcm` renders as fast as it can, so mpv may finish A before the
    // next entry reaches it (then A simply ends); a few tries settle that.
    for attempt in 1..=5 {
        let out = dir.path().join(format!("out{attempt}.raw"));
        let mut handle = mpv::spawn(MpvOptions {
            extra_args: vec![
                "--ao=pcm".to_owned(),
                format!("--ao-pcm-file={}", out.display()),
                "--ao-pcm-waveheader=no".to_owned(),
            ],
            ..MpvOptions::default()
        })
        .await
        .expect("mpv starts");
        handle.send(load(1, &a));
        handle.send(set_next(1, 2, &b));
        let (end, seen) = wait_for(&mut handle, |e| {
            matches!(
                e,
                PlaybackEvent::TrackEnded {
                    load: LoadId(2),
                    ..
                } | PlaybackEvent::TrackEnded {
                    load: LoadId(1),
                    ..
                }
            )
        })
        .await;
        assert_eq!(handle.shutdown(STEP).await, ShutdownOutcome::Clean);
        if !seen.contains(&ADVANCED) {
            eprintln!("attempt {attempt}: A ended before B was held: {end:?}");
            continue;
        }
        assert_eq!(
            end,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                reason: EndReason::Eof
            }
        );
        assert!(
            seen.contains(&PlaybackEvent::Started { load: LoadId(2) }),
            "{seen:?}"
        );
        let rendered = std::fs::read(&out).expect("pcm output");
        assert_eq!(rendered.len(), reference.len());
        let first_difference = rendered.iter().zip(&reference).position(|(x, y)| x != y);
        assert_eq!(first_difference, None, "the join at byte {}", cut * 2);
        return;
    }
    panic!("mpv never held the next track in time");
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn a_gapless_change_is_reported_and_the_next_track_starts_when_heard() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    common::write_tones(&a, 44100, 2, &[(440.0, 2000)], 0.2);
    common::write_tones(&b, 44100, 2, &[(880.0, 1000)], 0.2);
    let mut handle = timed_backend().await;
    let start = tokio::time::Instant::now();
    handle.send(load(1, &a));
    handle.send(set_next(1, 2, &b));
    let seen = timed_until(&mut handle, start, |e| {
        matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                ..
            }
        )
    })
    .await;
    let events: Vec<&PlaybackEvent> = seen.iter().map(|(_, e)| e).collect();
    assert!(
        !events.iter().any(|e| matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(1),
                ..
            }
        )),
        "A ends in the advance, not on its own: {seen:?}"
    );
    assert_eq!(
        seen.last().unwrap().1,
        PlaybackEvent::TrackEnded {
            load: LoadId(2),
            reason: EndReason::Eof
        }
    );
    let started_a = at(&seen, &PlaybackEvent::Started { load: LoadId(1) });
    let advanced = at(&seen, &ADVANCED);
    let started_b = at(&seen, &PlaybackEvent::Started { load: LoadId(2) });
    eprintln!("A started {started_a:.3}, advanced {advanced:.3}, B heard {started_b:.3}");
    // mpv moves on when A's decoding ends, the output's buffer before A is
    // heard to its end (about 0.35-0.45 s with `--ao=null`); B's `Started`
    // waits for that.
    assert!(started_a < advanced && advanced < started_b, "{seen:?}");
    let heard_after = started_b - advanced;
    assert!((0.2..0.8).contains(&heard_after), "{heard_after:.3} s");
    // B's start is where A's 2 s end, measured from A's start.
    let gap = started_b - started_a - 2.0;
    assert!((-0.15..0.25).contains(&gap), "{gap:.3} s");
    // No position of B before it is heard.
    let positions_between = seen
        .iter()
        .filter(|(t, e)| {
            *t > advanced
                && *t < started_b
                && matches!(e, PlaybackEvent::Position(p) if !p.is_zero())
        })
        .count();
    assert_eq!(positions_between, 0, "{seen:?}");
    handle.shutdown(STEP).await;
}

/// Plays `[A (2 s), B (2 s), C (1 s)]` through the player and real mpv,
/// removing B `edit_after` seconds after A is heard. With `deaf_from`, the
/// player reads no events from that many seconds after A is heard until
/// the edit, as if it were busy: mpv's advance to B then reaches it only
/// after the edit. Returns the titles started for the player and how the
/// change happened.
async fn remove_next_during_playback(
    edit_after: f64,
    deaf_from: Option<f64>,
) -> (Vec<String>, Player, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut player = Player::new();
    let mut ids = Vec::new();
    for (name, millis) in [("A", 2000), ("B", 2000), ("C", 1000)] {
        let path = dir.path().join(format!("{name}.wav"));
        common::write_tones(&path, 44100, 2, &[(440.0, millis)], 0.2);
        ids.push(
            player
                .enqueue(Track::local(path, TrackMetadata::default()))
                .unwrap(),
        );
    }
    let mut handle = timed_backend().await;
    let mut started = Vec::new();
    let commands = player.play_entry(ids[0]);
    handle.send_all(resolve::dispatch(&mut player, None, commands));
    let mut edit_at = None;
    let mut deaf_at: Option<tokio::time::Instant> = None;
    let mut how = "loaded after A ended".to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out: {started:?}"
        );
        let deaf = edit_at.is_some() && deaf_at.is_some_and(|at| tokio::time::Instant::now() >= at);
        let event = tokio::select! {
            event = handle.recv(), if !deaf => event.expect("backend still running"),
            _ = tokio::time::sleep_until(deaf_at.unwrap_or(deadline)), if edit_at.is_some() && !deaf && deaf_at.is_some() => continue,
            _ = tokio::time::sleep_until(edit_at.unwrap_or(deadline)), if edit_at.is_some() => {
                edit_at = None;
                // B may be the current entry already (mpv moved on, B is
                // not heard yet): then C replaces it at once.
                let commands = player.remove(ids[1]);
                handle.send_all(resolve::dispatch(&mut player, None, commands));
                continue;
            }
        };
        let is_start = matches!(&event, PlaybackEvent::Started { load } if player.current_load() == Some(*load));
        let advanced = match &event {
            PlaybackEvent::Advanced { from, to } if player.current_load() == Some(*from) => {
                Some(*to)
            }
            _ => None,
        };
        let commands = player.handle_event(event);
        if let Some(to) = advanced {
            how = if player.current_load() == Some(to) {
                let title = &player.queue().current_entry().unwrap().track.title;
                format!("gapless to {title}")
            } else {
                "corrected a stale advance".to_owned()
            };
        }
        if is_start {
            let title = player.queue().current_entry().unwrap().track.title.clone();
            if started.is_empty() {
                let now = tokio::time::Instant::now();
                edit_at = Some(now + Duration::from_secs_f64(edit_after));
                deaf_at = deaf_from.map(|d| now + Duration::from_secs_f64(d));
            }
            started.push(title);
        }
        handle.send_all(resolve::dispatch(&mut player, None, commands));
        if started.len() == 2 || (started.len() == 1 && player.status() == PlaybackStatus::Stopped)
        {
            break;
        }
    }
    handle.shutdown(STEP).await;
    (started, player, how)
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn a_next_track_removed_near_the_change_never_plays() {
    // mpv decodes A to its end about 0.4 s before it is heard to its end
    // (`--ao=null`), so removals from 1.2 s to 1.95 s after A's start cover
    // "well before", "around mpv's choice" and "after mpv moved on, before
    // B is heard".
    let mut paths = Vec::new();
    for edit_after in [1.2, 1.5, 1.58, 1.6, 1.62, 1.65, 1.7, 1.8, 1.95] {
        let (started, player, how) = remove_next_during_playback(edit_after, None).await;
        eprintln!("edit after {edit_after} s: {started:?}, {how}");
        assert_eq!(started, ["A", "C"], "edit after {edit_after} s");
        assert_eq!(player.queue().len(), 2);
        paths.push(how);
    }
    // Well before mpv's choice, the edit is taken without a gap.
    assert_eq!(paths[0], "gapless to C", "{paths:?}");
    // Deterministically in the switch window: mpv has moved on to B
    // (about 1.6 s), but the player has not seen it when B is removed
    // (1.9 s, B is heard at 2 s). It corrects the stale advance.
    let (started, _, how) = remove_next_during_playback(1.9, Some(1.0)).await;
    eprintln!("edit after 1.9 s, events held from 1.0 s: {started:?}, {how}");
    assert_eq!(started, ["A", "C"]);
    assert_eq!(how, "corrected a stale advance");
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn a_slow_stream_joins_without_a_gap_thanks_to_prefetch() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    // B's server takes 1 s to answer. mpv fetches it while A plays, so the
    // change does not wait for it.
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let b_wav = dir.path().join("b.wav");
    common::write_tones(&b_wav, 44100, 2, &[(880.0, 1000)], 0.2);
    Mock::given(method("GET"))
        .and(path("/music/rest/stream"))
        .and(query_param("id", "slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(std::fs::read(&b_wav).unwrap(), "audio/wav")
                .set_delay(Duration::from_secs(1)),
        )
        .mount(&server)
        .await;
    let client = navidrome::Client::new(
        navidrome::client::parse_base_url(&format!("{}/music", server.uri())).unwrap(),
        "u".into(),
        navidrome::Password::new("p".into()),
    )
    .unwrap();
    let a = dir.path().join("a.wav");
    common::write_tones(&a, 44100, 2, &[(440.0, 3000)], 0.2);
    let mut handle = timed_backend().await;
    let start = tokio::time::Instant::now();
    handle.send(load(1, &a));
    handle.send(PlaybackCommand::SetNext {
        after: LoadId(1),
        next: Some((
            LoadId(2),
            PlaybackTarget::Stream(client.stream_url("slow").unwrap()),
        )),
    });
    let seen = timed_until(&mut handle, start, |e| {
        matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                ..
            }
        )
    })
    .await;
    assert_eq!(
        seen.last().unwrap().1,
        PlaybackEvent::TrackEnded {
            load: LoadId(2),
            reason: EndReason::Eof
        },
        "{seen:?}"
    );
    let started_a = at(&seen, &PlaybackEvent::Started { load: LoadId(1) });
    let advanced = at(&seen, &ADVANCED);
    let started_b = at(&seen, &PlaybackEvent::Started { load: LoadId(2) });
    eprintln!("A started {started_a:.3}, advanced {advanced:.3}, B heard {started_b:.3}");
    // Without prefetch, B would be heard only after its 1 s latency.
    assert!(started_b - advanced < 0.6, "{seen:?}");
    let gap = started_b - started_a - 3.0;
    assert!((-0.15..0.3).contains(&gap), "{gap:.3} s: {seen:?}");
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.is_empty());
    handle.shutdown(STEP).await;
}

#[tokio::test]
#[ignore = "needs mpv"]
async fn the_visualizer_carries_on_across_a_gapless_change() {
    // A is a 400 Hz tone (band 2), B 2.5 kHz (band 4).
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    common::write_tones(&a, 44100, 2, &[(400.0, 2500)], 0.25);
    common::write_tones(&b, 44100, 2, &[(2500.0, 2000)], 0.25);
    let mut handle = timed_backend().await;
    let start = tokio::time::Instant::now();
    handle.send(PlaybackCommand::SetVisualizer(true));
    handle.send(load(1, &a));
    handle.send(set_next(1, 2, &b));
    let seen = timed_until(&mut handle, start, |e| {
        matches!(
            e,
            PlaybackEvent::TrackEnded {
                load: LoadId(2),
                ..
            } | PlaybackEvent::VisualizerUnavailable
        )
    })
    .await;
    assert!(
        !seen
            .iter()
            .any(|(_, e)| *e == PlaybackEvent::VisualizerUnavailable),
        "{seen:?}"
    );
    let advanced = at(&seen, &ADVANCED);
    let started_b = at(&seen, &PlaybackEvent::Started { load: LoadId(2) });
    let frames: Vec<(f64, LoadId, usize)> = seen
        .iter()
        .filter_map(|(t, e)| match e {
            PlaybackEvent::Visualizer { load, frame } => Some((*t, *load, loudest_band(frame))),
            _ => None,
        })
        .collect();
    // A's readings until B is heard, then B's.
    for &(t, load, band) in &frames {
        if load == LoadId(1) {
            assert!(t <= started_b + 0.01, "A's reading at {t:.3}: {frames:?}");
            assert_eq!(band, 2, "A's reading at {t:.3} shows B: {frames:?}");
        } else {
            assert_eq!(load, LoadId(2));
            assert!(t >= started_b, "B's reading at {t:.3} before it is heard");
        }
    }
    // Readings never stop across the change (they come about every 45 ms).
    let around: Vec<f64> = frames
        .iter()
        .map(|&(t, _, _)| t)
        .filter(|t| (advanced - 0.5..started_b + 0.5).contains(t))
        .collect();
    let longest = around.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    eprintln!(
        "advanced {advanced:.3}, B heard {started_b:.3}, {} readings around, longest pause {longest:.3} s",
        around.len()
    );
    assert!(longest < 0.2, "{frames:?}");
    // B's own band shows once it is heard.
    let b_bands: Vec<usize> = frames
        .iter()
        .filter(|&&(_, load, _)| load == LoadId(2))
        .map(|&(_, _, band)| band)
        .collect();
    assert!(b_bands.len() > 10, "{frames:?}");
    assert!(b_bands.iter().filter(|&&b| b == 4).count() * 10 >= b_bands.len() * 9);
    handle.shutdown(STEP).await;
}
