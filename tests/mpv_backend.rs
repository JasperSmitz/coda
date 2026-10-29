//! Integration tests against a real mpv (`#[ignore]`d; run with
//! `cargo test -- --ignored`). WAV fixtures are generated per test.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use coda_player::model::{Track, TrackMetadata};
use coda_player::navidrome;
use coda_player::playback::mpv::{self, MpvOptions};
use coda_player::playback::{
    EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackHandle, PlaybackTarget,
    ShutdownOutcome,
};
use coda_player::player::{PlaybackStatus, Player, PlayerCommand};
use coda_player::resolve;
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
    loads: usize,
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
    };
    let mut titles: HashMap<LoadId, String> = HashMap::new();
    let send = |player: &mut Player,
                handle: &PlaybackHandle,
                commands: Vec<PlayerCommand>,
                driven: &mut Driven,
                titles: &mut HashMap<LoadId, String>| {
        // The app's own dispatch: unresolvable loads advance the queue.
        for command in resolve::dispatch(player, navidrome, commands) {
            if let PlaybackCommand::Load { load, .. } = &command {
                driven.loads += 1;
                let title = player.queue().current_entry().unwrap().track.title.clone();
                titles.insert(*load, title);
            }
            handle.send(command);
        }
    };
    send(player, handle, initial, &mut driven, &mut titles);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !done(player, &driven.started) {
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        let event = next_event(handle).await;
        if let PlaybackEvent::Started { load } = &event
            && player.current_load() == Some(*load)
        {
            driven.started.push(titles[load].clone());
        }
        let commands = player.handle_event(event);
        send(player, handle, commands, &mut driven, &mut titles);
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
    assert_eq!(driven.loads, names.len());
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
    assert_eq!(driven.loads, 4);
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
