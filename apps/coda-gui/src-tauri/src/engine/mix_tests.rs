//! The engine's mixes: start, top-up, ending on a replace or another
//! server, a pending mix superseded or failing (the mix that plays goes
//! on), the session, and Navidrome fetches and retries driven like the
//! loop does (`drive_mixes`, tagged results back through a channel; the
//! retry timer by calling `mix_retry` at `mix_retry_at`) against a local
//! `wiremock` server. No window, no mpv.

use super::*;
use crate::history::SourceKind;
use crate::mixes::{IngredientDto, MixInputDto};
use coda_core::mix::navidrome::{MAX_ATTEMPTS, RETRY_AFTER};
use coda_core::mix::{Ingredient, Mix, MixSource, SessionMix, store};
use coda_core::navidrome::Password;
use coda_core::navidrome::client::parse_base_url;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn track(n: usize, artist: &str, genre: &str) -> Track {
    Track::local(
        PathBuf::from(format!("/m/{artist}/{n}.flac")),
        TrackMetadata {
            title: Some(format!("{artist} {n}")),
            artist: Some(artist.into()),
            album: Some(format!("{artist} album {}", n % 3)),
            duration: Some(Duration::from_secs(200)),
            genres: vec![genre.into()],
            ..Default::default()
        },
    )
}

/// 40 Prog Rock songs by three artists, and 5 Jazz songs.
fn library() -> Vec<Track> {
    let mut tracks: Vec<Track> = (0..40)
        .map(|n| track(n, ["Yes", "Genesis", "King Crimson"][n % 3], "Prog Rock"))
        .collect();
    tracks.extend((0..5).map(|n| track(n, "Miles", "Jazz")));
    tracks
}

fn sources(client: Option<Client>) -> Sources {
    Sources {
        music_dir: Some(PathBuf::from("/m")),
        scrobble: false,
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

/// A started engine (playback available) with the library scanned.
fn local_engine(queue: Vec<Track>) -> (Engine, Instant) {
    let now = Instant::now();
    let mut engine = Engine::new(Arc::new(CoverService::new(None)));
    engine.start(sources(None), None, restored(queue, None), now);
    engine.handle_scan_event(ScanEvent::Batch(library()), now);
    (engine, now)
}

fn quick(name: &str, source: SourceKind, ingredients: Vec<IngredientDto>) -> Intent {
    Intent::PlayQuickMix {
        mix: MixInputDto {
            name: name.into(),
            source,
            ingredients,
        },
    }
}

fn genre(name: &str) -> IngredientDto {
    IngredientDto::Genre { name: name.into() }
}

fn loads(commands: &[PlaybackCommand]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, PlaybackCommand::Load { .. }))
        .count()
}

fn queue_keys(engine: &mut Engine) -> Vec<String> {
    engine
        .full_snapshot()
        .queue
        .unwrap()
        .into_iter()
        .map(|i| i.key)
        .collect()
}

/// What the loop does with a `playMix`: read the file (here, at once).
fn read_load(engine: &mut Engine) -> LoadedMix {
    let load = engine.take_mix_load().expect("a mix to read");
    LoadedMix {
        serial: load.serial,
        context: load.context,
        read: read_saved_mix(&load.path, &load.id),
    }
}

fn remote_ref(song: &str) -> TrackRef {
    TrackRef {
        source: SourceDto::Navidrome {
            song_id: song.into(),
            cover_art: None,
        },
        title: Some(song.into()),
        artist: None,
        album: None,
        album_artist: None,
        track_no: None,
        disc_no: None,
        duration_ms: Some(120_000),
        album_id: None,
        artist_id: None,
        genres: Vec::new(),
    }
}

fn has_notice(engine: &mut Engine, text: &str) -> bool {
    notices(engine).iter().any(|n| n == text)
}

fn notices(engine: &mut Engine) -> Vec<String> {
    engine
        .full_snapshot()
        .notices
        .into_iter()
        .map(|n| n.text)
        .collect()
}

#[test]
fn a_local_mix_starts_tops_up_and_ends_when_the_queue_is_replaced() {
    let (mut engine, now) = local_engine(Vec::new());
    let commands = engine.handle_intent(
        quick(
            "Prog",
            SourceKind::Local,
            vec![genre("prog rock"), genre("Polka")],
        ),
        now,
    );
    assert_eq!(loads(&commands), 1, "plays the first song");
    let keys = queue_keys(&mut engine);
    assert_eq!(keys.len(), coda_core::mix::START_TRACKS);
    let mix = engine.full_snapshot().mix.unwrap();
    assert_eq!(
        (mix.name.as_str(), mix.id, mix.loading),
        ("Prog", None, false)
    );
    let texts = notices(&mut engine);
    assert!(texts.contains(&"Playing mix Prog".to_owned()), "{texts:?}");
    assert!(
        texts.contains(&"No songs for genre Polka".to_owned()),
        "{texts:?}"
    );
    // Every song is Prog Rock.
    let queue = engine.full_snapshot().queue.unwrap();
    assert!(queue.iter().all(|i| i.track.genres == ["Prog Rock"]));

    // Five or fewer left after the current entry: 20 more, gapless next
    // announced again.
    let commands = engine.handle_intent(
        Intent::PlayEntry {
            key: keys[20].clone(),
        },
        now,
    );
    assert_eq!(loads(&commands), 1);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, PlaybackCommand::SetNext { .. }))
    );
    assert_eq!(queue_keys(&mut engine).len(), 45);

    // The session saves the quick mix's recipe.
    let session = engine.take_final_save().unwrap();
    match session.mix {
        Some(SessionMix::Quick(mix)) => assert_eq!(mix.name, "Prog"),
        other => panic!("{other:?}"),
    }

    // Adding, moving and skipping keep it; clearing ends it.
    engine.handle_intent(Intent::Next, now);
    engine.handle_intent(Intent::ShuffleUpcoming, now);
    assert!(engine.full_snapshot().mix.is_some());
    engine.handle_intent(Intent::ClearQueue, now);
    assert!(engine.full_snapshot().mix.is_none());
    assert_eq!(engine.take_final_save().unwrap().mix, None);
}

#[test]
fn playing_something_else_ends_the_mix() {
    let (mut engine, now) = local_engine(Vec::new());
    engine.handle_intent(quick("Jazz", SourceKind::Local, vec![genre("Jazz")]), now);
    assert!(engine.full_snapshot().mix.is_some());
    engine.handle_intent(Intent::ShuffleLibrary, now);
    assert!(engine.full_snapshot().mix.is_none());
    let len = queue_keys(&mut engine).len();
    // No more top-ups for it.
    let last = queue_keys(&mut engine).pop().unwrap();
    engine.handle_intent(Intent::PlayEntry { key: last }, now);
    assert_eq!(queue_keys(&mut engine).len(), len);
}

#[test]
fn a_mix_with_no_songs_says_so_and_leaves_the_queue() {
    let (mut engine, now) = local_engine(Vec::new());
    engine.handle_intent(quick("Jazz", SourceKind::Local, vec![genre("Jazz")]), now);
    let before = queue_keys(&mut engine);
    let commands = engine.handle_intent(
        quick(
            "Nothing",
            SourceKind::Local,
            vec![
                genre("Polka"),
                IngredientDto::Artist {
                    name: "Nobody".into(),
                    id: None,
                },
            ],
        ),
        now,
    );
    assert_eq!(loads(&commands), 0);
    assert_eq!(queue_keys(&mut engine), before);
    // The mix that played goes on.
    assert_eq!(engine.full_snapshot().mix.unwrap().name, "Jazz");
    assert!(notices(&mut engine).contains(&"No songs for genre Polka, artist Nobody".to_owned()));
    // A recipe the model refuses is a notice too.
    engine.handle_intent(quick(" ", SourceKind::Local, vec![genre("Jazz")]), now);
    assert!(notices(&mut engine).contains(&"A mix needs a name".to_owned()));
    // A Navidrome mix without a server.
    engine.handle_intent(quick("x", SourceKind::Navidrome, vec![genre("Jazz")]), now);
    assert!(notices(&mut engine).contains(&"Not connected to Navidrome".to_owned()));
}

#[test]
fn a_saved_mix_is_read_again_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixes.json");
    let mix = Mix::new(
        "Big three",
        MixSource::Local,
        vec![
            Ingredient::Artist {
                name: "Yes".into(),
                id: None,
            },
            Ingredient::Artist {
                name: "genesis".into(),
                id: None,
            },
        ],
    )
    .unwrap();
    store::add(&path, mix.clone()).unwrap();
    let (mut engine, now) = local_engine(Vec::new());
    engine.set_mixes_path(Some(path.clone()));

    engine.handle_intent(
        Intent::PlayMix {
            id: mix.id.to_string(),
        },
        now,
    );
    let loaded = read_load(&mut engine);
    let commands = engine.mix_loaded(loaded, now);
    assert_eq!(loads(&commands), 1);
    let active = engine.full_snapshot().mix.unwrap();
    assert_eq!(active.id.as_deref(), Some(mix.id.as_str()));
    let queue = engine.full_snapshot().queue.unwrap();
    assert!(
        queue
            .iter()
            .all(|i| matches!(i.track.artist.as_deref(), Some("Yes" | "Genesis")))
    );
    assert_eq!(
        engine.take_final_save().unwrap().mix,
        Some(SessionMix::Saved(mix.id.clone()))
    );

    // A read overtaken by another play does not start.
    engine.handle_intent(
        Intent::PlayMix {
            id: mix.id.to_string(),
        },
        now,
    );
    let loaded = read_load(&mut engine);
    engine.handle_intent(Intent::ClearQueue, now);
    assert!(engine.mix_loaded(loaded, now).is_empty());
    assert!(engine.full_snapshot().mix.is_none());

    // A deleted mix (or a bad id) is a notice.
    store::delete(&path, &mix.id, &MixSource::Local).unwrap();
    engine.handle_intent(
        Intent::PlayMix {
            id: mix.id.to_string(),
        },
        now,
    );
    let loaded = read_load(&mut engine);
    assert!(engine.mix_loaded(loaded, now).is_empty());
    engine.handle_intent(Intent::PlayMix { id: "../x".into() }, now);
    assert!(engine.take_mix_load().is_none());
    assert!(notices(&mut engine).contains(&"That mix no longer exists".to_owned()));
}

#[test]
fn a_newer_mixes_file_is_said_so_not_taken_for_a_deleted_mix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixes.json");
    let mix = Mix::new(
        "J",
        MixSource::Local,
        vec![Ingredient::Genre {
            name: "Jazz".into(),
        }],
    )
    .unwrap();
    store::add(&path, mix.clone()).unwrap();
    // A newer Coda rewrote the file.
    std::fs::write(&path, r#"{"version": 99, "mixes": []}"#).unwrap();
    let (mut engine, now) = local_engine(Vec::new());
    engine.set_mixes_path(Some(path.clone()));
    engine.handle_intent(
        Intent::PlayMix {
            id: mix.id.to_string(),
        },
        now,
    );
    let loaded = read_load(&mut engine);
    assert_eq!(loaded.read, ReadMix::Newer);
    assert!(engine.mix_loaded(loaded, now).is_empty());
    let texts = notices(&mut engine);
    assert!(
        texts.contains(&"The mixes file was written by a newer version of Coda".to_owned()),
        "{texts:?}"
    );
    assert!(!texts.contains(&"That mix no longer exists".to_owned()));
    // On startup the session's mix is not resumed (and logged).
    let saved = Some(SessionMix::Saved(mix.id.clone()));
    assert_eq!(
        crate::bootstrap::restore_mix(saved, Some(&path)),
        (None, None)
    );
}

/// A restarted engine with a local Jazz mix restored on its queue
/// (current near the end) and no scan yet; and the queue's length.
fn restored_jazz_engine() -> (Engine, Instant, usize) {
    let (mut first, now) = local_engine(Vec::new());
    first.handle_intent(quick("Jazz", SourceKind::Local, vec![genre("Jazz")]), now);
    let session = first.take_final_save().unwrap();
    let Some(SessionMix::Quick(mix)) = session.mix.clone() else {
        panic!("{:?}", session.mix);
    };
    // Restart: the saved queue (current near the end), no scan yet.
    let mut engine = Engine::new(Arc::new(CoverService::new(None)));
    let current = session.tracks.len() - 1;
    engine.start(
        sources(None),
        None,
        restored(session.tracks.clone(), Some(current)),
        now,
    );
    engine.restore_mix(Some((mix, false)), now);
    let restored_len = queue_keys(&mut engine).len();
    (engine, now, restored_len)
}

#[test]
fn a_restored_mix_adopts_the_queue_and_tops_up_once_the_library_is_scanned() {
    let (mut engine, now, restored_len) = restored_jazz_engine();
    let active = engine.full_snapshot().mix.unwrap();
    assert_eq!((active.name.as_str(), active.loading), ("Jazz", false));
    // Restoring never plays, and with no library nothing is added.
    engine.handle_intent(Intent::CycleRepeat, now);
    assert_eq!(queue_keys(&mut engine).len(), restored_len);
    assert!(engine.take_final_save().is_some_and(|s| s.mix.is_some()));
    // The scan batch itself tops it up (without playing): no intent or
    // playback event needed.
    let commands = engine.handle_scan_event(ScanEvent::Batch(library()), now);
    assert_eq!(loads(&commands), 0);
    assert_eq!(queue_keys(&mut engine).len(), restored_len + 20);

    // Without a queue there is nothing to adopt.
    let mut empty = Engine::new(Arc::new(CoverService::new(None)));
    empty.start(sources(None), None, restored(Vec::new(), None), now);
    let jazz = Mix::new(
        "J",
        MixSource::Local,
        vec![Ingredient::Genre {
            name: "Jazz".into(),
        }],
    )
    .unwrap();
    empty.restore_mix(Some((jazz, true)), now);
    assert!(empty.full_snapshot().mix.is_none());
}

#[test]
fn a_restored_mix_tops_up_when_a_deferred_scan_batch_lands_on_a_tick() {
    let (mut engine, now, restored_len) = restored_jazz_engine();
    // A browse read holds the library: the batch waits for a tick.
    let browse = engine.browse().clone();
    let (held, release) = (
        Arc::new(std::sync::Barrier::new(2)),
        Arc::new(std::sync::Barrier::new(2)),
    );
    let reader = {
        let (held, release) = (held.clone(), release.clone());
        std::thread::spawn(move || {
            browse.read_library(|_| {
                held.wait();
                release.wait();
            })
        })
    };
    held.wait();
    let commands = engine.handle_scan_event(ScanEvent::Batch(library()), now);
    assert!(commands.is_empty());
    assert_eq!(engine.library_ops_pending(), 1);
    assert_eq!(queue_keys(&mut engine).len(), restored_len);
    release.wait();
    reader.join().unwrap();
    // The tick applies it and the mix tops up, without playing.
    let commands = engine.tick(now);
    assert_eq!(engine.library_ops_pending(), 0);
    assert_eq!(loads(&commands), 0);
    assert_eq!(queue_keys(&mut engine).len(), restored_len + 20);
}

#[test]
fn a_top_up_announces_the_song_it_added_as_the_next_one() {
    let (mut engine, now) = local_engine(Vec::new());
    engine.handle_intent(
        quick("Prog", SourceKind::Local, vec![genre("Prog Rock")]),
        now,
    );
    let keys = queue_keys(&mut engine);
    // The last entry: nothing follows it until the top-up adds songs.
    let commands = engine.handle_intent(
        Intent::PlayEntry {
            key: keys.last().unwrap().clone(),
        },
        now,
    );
    assert_eq!(loads(&commands), 1);
    let nexts: Vec<bool> = commands
        .iter()
        .filter_map(|c| match c {
            PlaybackCommand::SetNext { next, .. } => Some(next.is_some()),
            _ => None,
        })
        .collect();
    assert_eq!(
        nexts.last(),
        Some(&true),
        "the added song is announced: {nexts:?}"
    );
    assert_eq!(queue_keys(&mut engine).len(), keys.len() + 20);
}

#[test]
fn startup_reads_a_saved_mix_again_and_forgets_a_deleted_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixes.json");
    let mix = Mix::new(
        "J",
        MixSource::Local,
        vec![Ingredient::Genre {
            name: "Jazz".into(),
        }],
    )
    .unwrap();
    use crate::bootstrap::restore_mix;
    assert_eq!(
        restore_mix(Some(SessionMix::Quick(mix.clone())), None),
        (Some((mix.clone(), false)), None)
    );
    let saved = Some(SessionMix::Saved(mix.id.clone()));
    assert_eq!(restore_mix(saved.clone(), Some(&path)), (None, None));
    store::add(&path, mix.clone()).unwrap();
    assert_eq!(
        restore_mix(saved.clone(), Some(&path)),
        (Some((mix.clone(), true)), None)
    );
    assert_eq!(restore_mix(saved.clone(), None), (None, None));
    assert_eq!(restore_mix(None, Some(&path)), (None, None));
    // A mixes file that cannot be read is set aside and the user told.
    std::fs::write(&path, "not json").unwrap();
    let (resumed, notice) = restore_mix(saved, Some(&path));
    assert_eq!(resumed, None);
    let notice = notice.expect("a notice");
    assert!(
        notice.starts_with("Could not read the mixes file: ") && notice.contains("; kept as "),
        "{notice}"
    );
    let kept = notice.split("; kept as ").nth(1).unwrap();
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "not json");
    assert!(!path.exists());
}

// ----- Navidrome ---------------------------------------------------------------

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn songs(prefix: &str, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            json!({
                "id": format!("{prefix}-{i}"), "title": format!("Song {i}"),
                "album": format!("{prefix} {}", i % 4), "albumId": format!("{prefix}-al{}", i % 4),
                "artist": format!("Artist {}", i % 5)
            })
        })
        .collect()
}

fn navidrome_client(server: &MockServer, user: &str) -> Client {
    Client::new(
        parse_base_url(&server.uri()).unwrap(),
        user.into(),
        Password::new("s3cret-pass".into()),
    )
    .unwrap()
}

/// The loop's mix arm in a test: spawn what the engine wants (as
/// `drive_mixes` does) and apply the next fetch result.
struct Arm {
    fetched_tx: mpsc::UnboundedSender<MixFetched>,
    fetched_rx: mpsc::UnboundedReceiver<MixFetched>,
    loaded_tx: mpsc::UnboundedSender<LoadedMix>,
}

impl Arm {
    fn new() -> Self {
        let (fetched_tx, fetched_rx) = mpsc::unbounded_channel();
        let (loaded_tx, _) = mpsc::unbounded_channel();
        Self {
            fetched_tx,
            fetched_rx,
            loaded_tx,
        }
    }

    fn drive(&self, engine: &mut Engine) {
        drive_mixes(engine, &self.fetched_tx, &self.loaded_tx, Instant::now());
    }

    async fn next(&mut self, engine: &mut Engine) -> Vec<PlaybackCommand> {
        let (tag, fetched) = tokio::time::timeout(Duration::from_secs(10), self.fetched_rx.recv())
            .await
            .expect("a fetch result")
            .unwrap();
        let commands = engine.mix_fetched(tag, fetched, Instant::now());
        self.drive(engine);
        commands
    }
}

async fn genre_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Prog Rock"))
        .respond_with(ok(json!({ "randomSongs": { "song": songs("p", 60) } })))
        .mount(&server)
        .await;
    server
}

fn navidrome_engine(client: Client) -> Engine {
    let mut engine = Engine::new(Arc::new(CoverService::new(None)));
    let mut sources = sources(Some(client));
    sources.music_dir = None;
    engine.start(sources, None, restored(Vec::new(), None), Instant::now());
    engine
}

#[tokio::test]
async fn a_navidrome_mix_starts_once_its_songs_arrive_through_the_fetch_arm() {
    let server = genre_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let mut arm = Arm::new();
    let now = Instant::now();
    let commands = engine.handle_intent(
        quick("Prog", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    assert_eq!(loads(&commands), 0, "nothing plays before the songs arrive");
    let mix = engine.full_snapshot().mix.unwrap();
    assert!(mix.loading);
    assert!(queue_keys(&mut engine).is_empty());
    arm.drive(&mut engine);
    let commands = arm.next(&mut engine).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(queue_keys(&mut engine).len(), coda_core::mix::START_TRACKS);
    assert!(!engine.full_snapshot().mix.unwrap().loading);
    let session = engine.take_final_save().unwrap();
    assert!(matches!(session.mix, Some(SessionMix::Quick(_))));

    // Topping up from the pool: a sample smaller than a full request is
    // the whole genre, so playing it through asks for nothing more.
    for len in [45, 65, 85] {
        let keys = queue_keys(&mut engine);
        let last = keys[keys.len() - 2].clone();
        let commands = engine.handle_intent(Intent::PlayEntry { key: last }, now);
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, PlaybackCommand::SetNext { next: Some(_), .. })),
            "the next song is announced again"
        );
        arm.drive(&mut engine);
        assert_eq!(queue_keys(&mut engine).len(), len);
    }
    assert_eq!(engine.mix_played_through(), vec![0]);
    assert_eq!(engine.mix_fetches_in_flight(), 0);
    let asked = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/rest/getRandomSongs")
        .count();
    assert_eq!(asked, 1);
}

#[tokio::test]
async fn a_navidrome_mix_replaced_while_loading_never_starts() {
    let server = genre_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let mut arm = Arm::new();
    let now = Instant::now();
    engine.handle_intent(
        quick("Prog", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    arm.drive(&mut engine);
    // Adding a song keeps it loading.
    engine.handle_intent(
        Intent::Enqueue {
            tracks: vec![remote_ref("s1")],
            next: false,
        },
        now,
    );
    assert_eq!(queue_keys(&mut engine).len(), 1);
    assert!(engine.full_snapshot().mix.is_some_and(|m| m.loading));
    // The user clears the queue before the songs arrive.
    engine.handle_intent(Intent::ClearQueue, now);
    assert!(engine.full_snapshot().mix.is_none());
    let commands = arm.next(&mut engine).await;
    assert!(commands.is_empty());
    assert!(queue_keys(&mut engine).is_empty());
}

/// A server with Prog Rock (60 songs, at once), Jazz (30 songs, after
/// 300 ms), Polka (none) and Broken (always HTTP 500).
async fn mixed_server() -> MockServer {
    let server = genre_server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Jazz"))
        .respond_with(
            ok(json!({ "randomSongs": { "song": songs("j", 30) } }))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Polka"))
        .respond_with(ok(json!({ "randomSongs": {} })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Broken"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_navidrome_mix_replaced_by_another_while_loading_never_starts() {
    let server = mixed_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let mut arm = Arm::new();
    let now = Instant::now();
    engine.handle_intent(
        quick("A", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    arm.drive(&mut engine);
    engine.handle_intent(quick("B", SourceKind::Navidrome, vec![genre("Jazz")]), now);
    arm.drive(&mut engine);
    let b = engine.pending_mix_tag();
    assert!(b.is_some());
    let snapshot = engine.full_snapshot().mix.unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.loading), ("B", true));
    // A's result (its songs, or its stopped fetch) comes first: ignored.
    let commands = arm.next(&mut engine).await;
    assert!(commands.is_empty());
    assert!(queue_keys(&mut engine).is_empty());
    assert_eq!(engine.pending_mix_tag(), b, "B still loads");
    assert_eq!(engine.mix_fetches_in_flight(), 1, "B's fetch");
    assert_eq!(engine.playing_mix_name(), None);
    // B's songs start B.
    let commands = arm.next(&mut engine).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(engine.playing_mix_name().as_deref(), Some("B"));
    assert_eq!(engine.pending_mix_tag(), None);
    let queue = engine.full_snapshot().queue.unwrap();
    assert_eq!(queue.len(), coda_core::mix::START_TRACKS);
}

#[tokio::test]
async fn a_saved_mix_asked_for_while_another_loads_is_the_one_that_starts() {
    let server = genre_server().await;
    let client = navidrome_client(&server, "listener");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixes.json");
    let b = Mix::new(
        "B",
        MixSource::Navidrome {
            server: coda_core::navidrome::server_id(&client),
        },
        vec![Ingredient::Genre {
            name: "Prog Rock".into(),
        }],
    )
    .unwrap();
    store::add(&path, b.clone()).unwrap();
    let mut engine = navidrome_engine(client);
    engine.set_mixes_path(Some(path));
    let mut arm = Arm::new();
    let now = Instant::now();
    // A loads; then the user plays saved mix B, whose file read is slow.
    engine.handle_intent(
        quick("A", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    arm.drive(&mut engine);
    engine.handle_intent(
        Intent::PlayMix {
            id: b.id.to_string(),
        },
        now,
    );
    let loaded = read_load(&mut engine);
    assert!(engine.full_snapshot().mix.is_none(), "A no longer loads");
    // A's result lands before B's read: A does not start.
    let commands = arm.next(&mut engine).await;
    assert!(commands.is_empty());
    assert!(queue_keys(&mut engine).is_empty());
    // B's read: B loads, then starts.
    assert!(engine.mix_loaded(loaded, now).is_empty());
    let snapshot = engine.full_snapshot().mix.unwrap();
    assert_eq!(
        (snapshot.name.as_str(), snapshot.id, snapshot.loading),
        ("B", Some(b.id.to_string()), true)
    );
    arm.drive(&mut engine);
    let commands = arm.next(&mut engine).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(engine.playing_mix_name().as_deref(), Some("B"));
    assert_eq!(
        engine.take_final_save().unwrap().mix,
        Some(SessionMix::Saved(b.id.clone()))
    );
}

#[tokio::test]
async fn a_play_request_that_finds_nothing_still_supersedes_a_loading_mix() {
    let server = genre_server().await;
    let client = navidrome_client(&server, "listener");
    let now = Instant::now();
    let mut engine = Engine::new(Arc::new(CoverService::new(None)));
    engine.start(sources(Some(client)), None, restored(Vec::new(), None), now);
    engine.handle_scan_event(ScanEvent::Batch(library()), now);
    let mut arm = Arm::new();
    engine.handle_intent(
        quick("A", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    arm.drive(&mut engine);
    assert!(engine.pending_mix_tag().is_some());
    // An album from a view of an older library: every ref is stale, so
    // nothing plays, but the user asked for something else than A.
    let mut stale = remote_ref("x");
    stale.source = SourceDto::Local {
        id: Some(crate::browse::local_id(999, 0)),
    };
    let commands = engine.handle_intent(
        Intent::PlayTracks {
            tracks: vec![stale],
            start: None,
            shuffle: false,
            context: None,
        },
        now,
    );
    assert_eq!(loads(&commands), 0);
    assert!(has_notice(
        &mut engine,
        "The library changed; 1 track skipped. Reload the view"
    ));
    assert_eq!(engine.pending_mix_tag(), None);
    assert!(engine.full_snapshot().mix.is_none());
    // A's songs arrive: ignored.
    let commands = arm.next(&mut engine).await;
    assert!(commands.is_empty());
    assert!(queue_keys(&mut engine).is_empty());
    assert_eq!(engine.playing_mix_name(), None);
}

#[tokio::test]
async fn a_refused_mix_request_leaves_the_loading_mix() {
    let server = genre_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let now = Instant::now();
    engine.handle_intent(
        quick("A", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    let a = engine.pending_mix_tag();
    assert!(a.is_some());
    // An invalid quick mix (no ingredients).
    engine.handle_intent(quick("B", SourceKind::Navidrome, Vec::new()), now);
    assert_eq!(engine.pending_mix_tag(), a);
    // A saved mix with a malformed id.
    engine.handle_intent(Intent::PlayMix { id: "nope".into() }, now);
    assert!(engine.take_mix_load().is_none());
    assert_eq!(engine.pending_mix_tag(), a);
    // A Navidrome quick mix with no client.
    let client = engine.client.take();
    engine.handle_intent(
        quick("C", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    assert!(has_notice(&mut engine, "Not connected to Navidrome"));
    assert_eq!(engine.pending_mix_tag(), a);
    engine.client = client;
    // A valid one supersedes it.
    engine.handle_intent(
        quick("D", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    assert!(engine.pending_mix_tag().is_some_and(|d| Some(d) != a));
}

/// A Navidrome engine playing quick mix `name` of `genres` (all
/// fetched).
async fn playing(server: &MockServer, arm: &mut Arm, name: &str, genres: &[&str]) -> Engine {
    let mut engine = navidrome_engine(navidrome_client(server, "listener"));
    engine.handle_intent(
        quick(
            name,
            SourceKind::Navidrome,
            genres.iter().map(|g| genre(g)).collect(),
        ),
        Instant::now(),
    );
    arm.drive(&mut engine);
    for _ in genres {
        arm.next(&mut engine).await;
    }
    assert_eq!(engine.playing_mix_name().as_deref(), Some(name));
    engine
}

#[tokio::test]
async fn a_replacement_with_no_songs_leaves_the_mix_that_plays_and_saves_each_change() {
    let server = mixed_server().await;
    let mut arm = Arm::new();
    let mut engine = playing(&server, &mut arm, "A", &["Prog Rock"]).await;
    let now = Instant::now();
    assert!(
        engine
            .take_due_save(now + Duration::from_secs(60))
            .is_some()
    );
    let revision = engine.player.revision();
    let queue = queue_keys(&mut engine);

    engine.handle_intent(
        quick("Nothing", SourceKind::Navidrome, vec![genre("Polka")]),
        now,
    );
    let snapshot = engine.full_snapshot().mix.unwrap();
    assert_eq!(
        (snapshot.name.as_str(), snapshot.loading),
        ("Nothing", true)
    );
    // Loading is a session change of its own (the queue did not change).
    assert_eq!(engine.player.revision(), revision);
    let session = engine.take_due_save(now + SAVE_DEBOUNCE).expect("a save");
    assert!(matches!(session.mix, Some(SessionMix::Quick(m)) if m.name == "A"));

    arm.drive(&mut engine);
    let commands = arm.next(&mut engine).await;
    assert_eq!(loads(&commands), 0);
    // A goes on, with its queue; said so.
    assert_eq!(engine.player.revision(), revision);
    assert_eq!(queue_keys(&mut engine), queue);
    let snapshot = engine.full_snapshot().mix.unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.loading), ("A", false));
    assert!(has_notice(&mut engine, "No songs for genre Polka"));
    let session = engine
        .take_due_save(Instant::now() + SAVE_DEBOUNCE)
        .expect("the failure is saved too");
    assert!(matches!(session.mix, Some(SessionMix::Quick(m)) if m.name == "A"));
    // A still tops up.
    let keys = queue_keys(&mut engine);
    engine.handle_intent(
        Intent::PlayEntry {
            key: keys.last().unwrap().clone(),
        },
        now,
    );
    assert_eq!(queue_keys(&mut engine).len(), keys.len() + 20);
}

#[tokio::test]
async fn a_failed_fetch_is_tried_again_after_retry_after() {
    let server = genre_server().await;
    // The first request fails; later ones have songs.
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let mut arm = Arm::new();
    engine.handle_intent(
        quick("Prog", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        Instant::now(),
    );
    arm.drive(&mut engine);
    let before = Instant::now();
    let commands = arm.next(&mut engine).await;
    let after = Instant::now();
    assert_eq!(loads(&commands), 0);
    assert!(engine.full_snapshot().mix.unwrap().loading);
    assert!(queue_keys(&mut engine).is_empty());
    let at = engine.mix_retry_at().expect("a retry");
    assert!(at >= before + RETRY_AFTER && at <= after + RETRY_AFTER);
    // Not before then.
    engine.mix_retry(at - Duration::from_millis(1));
    assert_eq!(engine.mix_fetches_in_flight(), 0);
    assert!(engine.take_mix_fetches().is_empty());
    // Then: the fetch goes out and the mix starts.
    engine.mix_retry(at);
    assert_eq!(engine.mix_fetches_in_flight(), 1);
    assert_eq!(engine.mix_retry_at(), None, "nothing else waits");
    arm.drive(&mut engine);
    let commands = arm.next(&mut engine).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(engine.playing_mix_name().as_deref(), Some("Prog"));
    assert_eq!(engine.mix_retry_at(), None);
}

/// Retries until the fetches of the mixes have no retry left.
async fn retry_all(engine: &mut Engine, arm: &mut Arm) -> usize {
    let mut retries = 0;
    while let Some(at) = engine.mix_retry_at() {
        engine.mix_retry(at);
        arm.drive(engine);
        arm.next(engine).await;
        retries += 1;
        assert!(retries <= 10, "retries never end");
    }
    retries
}

#[tokio::test]
async fn failed_ingredients_are_said_and_given_up_after_max_attempts() {
    let server = mixed_server().await;
    let mut arm = Arm::new();
    // Broken never loads: the mix starts with Prog Rock, and says so.
    let mut engine = playing(&server, &mut arm, "Two", &["Prog Rock", "Broken"]).await;
    assert!(has_notice(
        &mut engine,
        "Could not load genre Broken; trying again later"
    ));
    let retries = retry_all(&mut engine, &mut arm).await;
    assert_eq!(retries as u32, MAX_ATTEMPTS - 1);
    assert_eq!(engine.playing_mix_name().as_deref(), Some("Two"));
    let queue = queue_keys(&mut engine);

    // A mix of Broken alone: loading through every attempt, then dropped
    // with a notice; Two goes on.
    let now = Instant::now();
    engine.handle_intent(
        quick("Broken", SourceKind::Navidrome, vec![genre("Broken")]),
        now,
    );
    arm.drive(&mut engine);
    arm.next(&mut engine).await;
    assert!(engine.full_snapshot().mix.unwrap().loading);
    let retries = retry_all(&mut engine, &mut arm).await;
    assert_eq!(retries as u32, MAX_ATTEMPTS - 1);
    assert!(has_notice(
        &mut engine,
        "Could not load the mix's songs (genre Broken)"
    ));
    assert_eq!(engine.pending_mix_tag(), None);
    assert_eq!(engine.mix_retry_at(), None, "no busy retry");
    assert_eq!(engine.playing_mix_name().as_deref(), Some("Two"));
    assert_eq!(queue_keys(&mut engine), queue);
}

#[tokio::test]
async fn no_server_ends_the_navidrome_mixes() {
    let server = mixed_server().await;
    let mut arm = Arm::new();
    let mut engine = playing(&server, &mut arm, "A", &["Prog Rock"]).await;
    let now = Instant::now();
    engine.handle_intent(quick("B", SourceKind::Navidrome, vec![genre("Jazz")]), now);
    // Disconnected: the mix that plays and the one loading end.
    engine
        .apply_settings(
            SettingsChange::Client {
                client: None,
                scrobble: false,
            },
            now,
        )
        .unwrap();
    assert!(engine.full_snapshot().mix.is_none());
    assert_eq!(engine.playing_mix_name(), None);
    assert!(has_notice(&mut engine, "Not connected to Navidrome"));
    assert_eq!(engine.take_final_save().unwrap().mix, None);
}

#[tokio::test]
async fn fetches_without_a_client_end_the_mix() {
    let server = genre_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let arm = Arm::new();
    engine.handle_intent(
        quick("Prog", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        Instant::now(),
    );
    // The client is gone by the time the loop spawns the fetches.
    engine.client = None;
    arm.drive(&mut engine);
    assert!(engine.full_snapshot().mix.is_none());
    assert!(engine.take_mix_fetches().is_empty());
    assert!(has_notice(&mut engine, "Not connected to Navidrome"));
}

#[tokio::test]
async fn another_server_ends_a_navidrome_mix() {
    let server = genre_server().await;
    let mut engine = navidrome_engine(navidrome_client(&server, "listener"));
    let mut arm = Arm::new();
    let now = Instant::now();
    engine.handle_intent(
        quick("Prog", SourceKind::Navidrome, vec![genre("Prog Rock")]),
        now,
    );
    arm.drive(&mut engine);
    arm.next(&mut engine).await;
    assert!(engine.full_snapshot().mix.is_some());
    // The account changes: the mix ends, the queue stays.
    let len = queue_keys(&mut engine).len();
    engine
        .apply_settings(
            SettingsChange::Client {
                client: Some(navidrome_client(&server, "other")),
                scrobble: false,
            },
            now,
        )
        .unwrap();
    assert!(engine.full_snapshot().mix.is_none());
    assert_eq!(queue_keys(&mut engine).len(), len);
    assert!(
        notices(&mut engine).contains(&"This mix was made on another Navidrome server".to_owned())
    );
    // Saved without the mix.
    assert_eq!(engine.take_final_save().unwrap().mix, None);

    // A mix of that server restored on another is not resumed.
    let mix = Mix::new(
        "Prog",
        MixSource::Navidrome {
            server: coda_core::navidrome::server_id(&navidrome_client(&server, "listener")),
        },
        vec![Ingredient::Genre {
            name: "Prog Rock".into(),
        }],
    )
    .unwrap();
    let mut other = Engine::new(Arc::new(CoverService::new(None)));
    let mut sources = sources(Some(navidrome_client(&server, "other")));
    sources.music_dir = None;
    other.start(
        sources,
        None,
        restored(vec![track(0, "Yes", "Prog Rock")], Some(0)),
        now,
    );
    other.restore_mix(Some((mix.clone(), true)), now);
    assert!(other.full_snapshot().mix.is_none());
    // On its own server it resumes and fetches its songs.
    let mut same_sources = self::sources(Some(navidrome_client(&server, "listener")));
    same_sources.music_dir = None;
    let mut resumed = Engine::new(Arc::new(CoverService::new(None)));
    resumed.start(
        same_sources,
        None,
        restored(vec![track(0, "Yes", "Prog Rock")], Some(0)),
        now,
    );
    resumed.restore_mix(Some((mix, true)), now);
    assert!(resumed.full_snapshot().mix.is_some());
    arm.drive(&mut resumed);
    arm.next(&mut resumed).await;
    // Topped up from the fetched songs, without playing.
    assert_eq!(queue_keys(&mut resumed).len(), 21);
}
