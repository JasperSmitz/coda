//! The TUI's mixes: the Mixes views (both sources, read from the shared
//! file), playing a saved mix, top-up, a replace ending it, quick mixes
//! from marked rows, the session, and Navidrome fetches driven like the
//! loop does (`drive_mixes`, tagged results back through a channel)
//! against a local `wiremock` server. No terminal, no mpv.

use std::time::Duration;

use super::*;
use coda_core::mix::navidrome::RETRY_AFTER;
use coda_core::mix::store;
use coda_core::model::TrackMetadata;
use coda_core::navidrome::api::RANDOM_SONGS_SIZE;
use coda_core::navidrome::browse::{Content, FetchResponse, Resource};
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Access, Album, AlbumPage, Artist, Password, Playlist, PlaylistSongs};
use coda_core::session::Session;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ROOT: &str = "/music";

fn track(n: usize, artist: &str, genre: &str) -> Track {
    Track::local(
        PathBuf::from(format!("{ROOT}/{artist}/{n}.flac")),
        TrackMetadata {
            title: Some(format!("{artist} {n}")),
            artist: Some(artist.into()),
            album: Some(format!("{artist} album {}", n % 3)),
            genres: vec![genre.into()],
            ..Default::default()
        },
    )
}

/// 40 Prog Rock songs by three artists, and 5 Jazz songs by Miles.
fn library() -> Vec<Track> {
    let mut tracks: Vec<Track> = (0..40)
        .map(|n| track(n, ["Yes", "Genesis", "King Crimson"][n % 3], "Prog Rock"))
        .collect();
    tracks.extend((0..5).map(|n| track(n, "Miles", "Jazz")));
    tracks
}

fn local_app(client: Option<navidrome::Client>) -> (App, Instant) {
    let now = Instant::now();
    let mut app = App::with_sources(Some(ROOT.into()), client);
    app.handle_scan_event(ScanEvent::Batch(library()), now);
    (app, now)
}

fn genre(name: &str) -> Ingredient {
    Ingredient::Genre { name: name.into() }
}

fn local_mix(name: &str, ingredients: Vec<Ingredient>) -> Mix {
    Mix::new(name, MixSource::Local, ingredients).unwrap()
}

fn press(app: &mut App, actions: &[Action], now: Instant) -> Vec<PlaybackCommand> {
    actions
        .iter()
        .flat_map(|a| app.handle_action(*a, now))
        .collect()
}

fn loads(commands: &[PlaybackCommand]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, PlaybackCommand::Load { .. }))
        .count()
}

fn queue_len(app: &App) -> usize {
    app.player().queue().len()
}

fn queue_titles(app: &App) -> Vec<String> {
    let queue = app.player().queue();
    queue
        .entries()
        .iter()
        .map(|e| e.track.title.clone())
        .collect()
}

/// Opens the local Home's `entry` (from Home).
fn open_local(app: &mut App, entry: LocalHomeEntry, now: Instant) {
    press(app, &[Action::SelectLocal], now);
    while app.local_browser().views().count() > 1 {
        press(app, &[Action::Parent], now);
    }
    let at = coda_core::library::browse::HOME_ENTRIES
        .iter()
        .position(|e| *e == entry)
        .unwrap();
    while app.library_selected() > at {
        press(app, &[Action::Up], now);
    }
    while app.library_selected() < at {
        press(app, &[Action::Down], now);
    }
    press(app, &[Action::Open], now);
}

/// What the loop does with a read of the mixes file (here, at once).
fn read_mixes(app: &mut App, now: Instant) {
    let read = app.take_mixes_read().expect("a read of the mixes file");
    app.mixes_loaded(read.read(), now);
}

/// What the loop does with a saved mix to read before it plays (here, at
/// once).
fn read_saved(app: &mut App, now: Instant) -> Vec<PlaybackCommand> {
    let load = app.take_mix_load().expect("a saved mix to read");
    app.mix_loaded(load.read(), now)
}

fn mixes_file(mixes: &[Mix]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixes.json");
    for mix in mixes {
        store::add(&path, mix.clone()).unwrap();
    }
    (dir, path)
}

fn status(app: &App, now: Instant) -> String {
    app.status(now).unwrap_or_default().to_owned()
}

/// Plays the entry `n` in the queue, as Enter in the Queue panel does.
fn play_queue_entry(app: &mut App, n: usize, now: Instant) -> Vec<PlaybackCommand> {
    if app.focus() != Focus::Queue {
        press(app, &[Action::SwitchFocus], now);
    }
    while app.queue_selected() < n {
        press(app, &[Action::Down], now);
    }
    while app.queue_selected() > n {
        press(app, &[Action::Up], now);
    }
    let commands = press(app, &[Action::Activate], now);
    press(app, &[Action::SwitchFocus], now);
    commands
}

#[test]
fn home_mixes_reads_the_file_and_enter_plays_a_saved_mix() {
    let other = navidrome::server_id_of("https://other.example/", "u");
    let ours = navidrome::server_id(&client());
    let prog = local_mix("Prog", vec![genre("prog rock")]);
    let remote = Mix::new(
        "Server mix",
        MixSource::Navidrome { server: ours },
        vec![genre("Jazz")],
    )
    .unwrap();
    let elsewhere = Mix::new(
        "Elsewhere",
        MixSource::Navidrome { server: other },
        vec![genre("Jazz")],
    )
    .unwrap();
    let (_dir, path) = mixes_file(&[prog.clone(), remote, elsewhere]);
    let (mut app, now) = local_app(Some(client()));
    app.set_mixes_path(Some(path));

    open_local(&mut app, LocalHomeEntry::Mixes, now);
    assert_eq!(*app.local_browser().current(), LocalView::Mixes);
    read_mixes(&mut app, now);
    let names: Vec<&str> = app
        .local_browser()
        .mixes()
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["Prog"], "only the local mixes");
    // Moving within the view reads nothing; `R` reads again.
    press(&mut app, &[Action::Down], now);
    assert!(app.take_mixes_read().is_none());
    press(&mut app, &[Action::Refresh], now);
    read_mixes(&mut app, now);

    // Enter reads the mix again (on the loop's blocking thread), then
    // plays it.
    assert!(press(&mut app, &[Action::Activate], now).is_empty());
    assert_eq!(queue_len(&app), 0);
    let commands = read_saved(&mut app, now);
    assert_eq!(loads(&commands), 1);
    assert_eq!(queue_len(&app), mix::START_TRACKS);
    assert!(
        queue_titles(&app).iter().all(|t| !t.starts_with("Miles")),
        "only Prog Rock: {:?}",
        queue_titles(&app)
    );
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog"));
    assert_eq!(status(&app, now), "Playing mix Prog");
    let session = app.take_final_save().unwrap();
    assert_eq!(session.mix, Some(SessionMix::Saved(prog.id.clone())));

    // The Navidrome Mixes view holds this server's mixes only.
    press(&mut app, &[Action::SelectNavidrome], now);
    let at = browse::HOME_ENTRIES
        .iter()
        .position(|e| *e == browse::HomeEntry::Mixes)
        .unwrap();
    for _ in 0..at {
        press(&mut app, &[Action::Down], now);
    }
    press(&mut app, &[Action::Activate], now);
    read_mixes(&mut app, now);
    let rows: Vec<String> = app
        .browser()
        .and_then(|b| b.current_content())
        .map(|c| {
            c.rows(0, 10)
                .into_iter()
                .filter_map(|r| match r {
                    browse::Row::Mix(m) => Some(m.name.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap();
    assert_eq!(rows, ["Server mix"]);
    assert!(app.take_fetch_requests().is_empty(), "never fetched");
}

#[test]
fn without_a_data_folder_mixes_say_so() {
    let (mut app, now) = local_app(None);
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    assert!(app.take_mixes_read().is_none());
    assert_eq!(status(&app, now), NO_MIXES_FILE);
}

#[test]
fn a_mix_tops_up_before_the_end_and_a_replace_ends_it() {
    let prog = local_mix("Prog", vec![genre("Prog Rock"), genre("Jazz")]);
    let (_dir, path) = mixes_file(&[prog]);
    let (mut app, now) = local_app(None);
    app.set_mixes_path(Some(path));
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    read_mixes(&mut app, now);
    // `s` plays a mix too.
    press(&mut app, &[Action::PlayShuffled], now);
    read_saved(&mut app, now);
    assert_eq!(queue_len(&app), mix::START_TRACKS);
    // Skipping to near the end tops the queue up, and the next track is
    // announced again.
    let commands = play_queue_entry(&mut app, 20, now);
    assert_eq!(queue_len(&app), mix::START_TRACKS + mix::TOP_UP_TRACKS);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, PlaybackCommand::SetNext { next: Some(_), .. })),
        "{commands:?}"
    );
    // Adding songs and moving on keeps the mix.
    let len = queue_len(&app);
    open_local(&mut app, LocalHomeEntry::Albums, now);
    press(&mut app, &[Action::Enqueue], now);
    assert!(queue_len(&app) > len);
    press(&mut app, &[Action::Next], now);
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog"));

    // Playing an album replaces the queue: the mix ends.
    press(&mut app, &[Action::Activate], now);
    assert_eq!(app.mix_label(), None);
    assert_eq!(app.take_final_save().unwrap().mix, None);
    // Near the end of the album, nothing is added.
    let len = queue_len(&app);
    play_queue_entry(&mut app, len - 1, now);
    assert_eq!(queue_len(&app), len);
}

#[test]
fn clearing_the_queue_ends_the_mix() {
    let (mut app, now) = local_app(None);
    let commands = app.play_mix(local_mix("Jazz", vec![genre("Jazz")]), false, now);
    app.dispatch(commands, now);
    assert_eq!(app.mix_label().as_deref(), Some("Quick mix: Jazz"));
    press(&mut app, &[Action::SwitchFocus, Action::ClearQueue], now);
    press(&mut app, &[Action::ClearQueue], now);
    assert_eq!(queue_len(&app), 0);
    assert_eq!(app.mix_label(), None);
}

#[test]
fn marked_rows_play_as_a_quick_mix_and_the_marks_clear() {
    let (mut app, now) = local_app(None);
    // Nothing marked yet; a Home entry cannot be marked.
    press(&mut app, &[Action::PlayMarked], now);
    assert_eq!(status(&app, now), NOTHING_MARKED);
    press(&mut app, &[Action::ToggleMark], now);
    assert_eq!(status(&app, now), NOT_MARKABLE);

    open_local(&mut app, LocalHomeEntry::Genres, now);
    // Genres A–Z: Jazz, Prog Rock.
    press(&mut app, &[Action::Down, Action::ToggleMark], now);
    assert_eq!(
        status(&app, now),
        "Marked genre Prog Rock (1 marked; M plays them as a mix)"
    );
    // A second `m` unmarks, a third marks again.
    press(&mut app, &[Action::ToggleMark], now);
    assert_eq!(status(&app, now), "Unmarked genre Prog Rock");
    press(&mut app, &[Action::ToggleMark], now);
    open_local(&mut app, LocalHomeEntry::Artists, now);
    while app.local_browser().selected_label(app.library()).as_deref() != Some("Miles") {
        press(&mut app, &[Action::Down], now);
    }
    press(&mut app, &[Action::ToggleMark], now);
    let marks = app.marks(Source::Local).unwrap();
    assert_eq!(
        marks.marked(),
        [
            genre("Prog Rock"),
            Ingredient::Artist {
                name: "Miles".into(),
                id: None
            }
        ]
    );
    // Marks are per source: Navidrome has none (and no client here).
    assert!(app.marks(Source::Navidrome).is_none());

    let commands = press(&mut app, &[Action::PlayMarked], now);
    assert_eq!(loads(&commands), 1);
    assert_eq!(
        app.mix_label().as_deref(),
        Some("Quick mix: Prog Rock + Miles")
    );
    assert!(app.marks(Source::Local).unwrap().is_empty());
    assert_eq!(queue_len(&app), mix::START_TRACKS);
    // Balanced: Miles has only 5 songs, and every one of them plays.
    let mut miles: Vec<String> = queue_titles(&app)
        .into_iter()
        .filter(|t| t.starts_with("Miles"))
        .collect();
    miles.sort();
    miles.dedup();
    assert_eq!(miles.len(), 5, "{:?}", queue_titles(&app));
    let session = app.take_final_save().unwrap();
    assert!(
        matches!(&session.mix, Some(SessionMix::Quick(m)) if m.name == "Prog Rock + Miles"),
        "{:?}",
        session.mix
    );
}

#[test]
fn a_mix_that_finds_nothing_leaves_the_queue_and_the_playing_mix() {
    let (mut app, now) = local_app(None);
    let commands = app.play_mix(local_mix("Jazz", vec![genre("Jazz")]), false, now);
    app.dispatch(commands, now);
    let before = queue_titles(&app);
    let commands = app.play_mix(local_mix("Polka", vec![genre("Polka")]), false, now);
    assert!(commands.is_empty());
    assert_eq!(status(&app, now), "No songs for genre Polka");
    assert_eq!(queue_titles(&app), before);
    assert_eq!(app.mix_label().as_deref(), Some("Quick mix: Jazz"));
}

#[test]
fn a_restored_mix_adopts_the_queue_and_tops_up_once_the_scan_has_songs() {
    let now = Instant::now();
    let mut app = App::new(ROOT.into());
    let session = Session {
        tracks: vec![
            track(0, "Yes", "Prog Rock"),
            track(1, "Genesis", "Prog Rock"),
        ],
        current: Some(0),
        ..Session::default()
    };
    app.restore_session(session, now);
    let prog = local_mix("Prog", vec![genre("Prog Rock")]);
    app.restore_mix(Some((prog.clone(), true)), now);
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog"));
    assert!(
        app.take_final_save().is_none(),
        "restored, so already saved"
    );
    // No songs scanned yet: nothing to add.
    press(&mut app, &[Action::Next], now);
    assert_eq!(queue_len(&app), 2);
    // The scan's first batch tops the queue up, and the commands it
    // returns (for the loop to send) announce the next track again.
    let commands = app.handle_scan_event(ScanEvent::Batch(library()), now);
    assert_eq!(queue_len(&app), 2 + mix::TOP_UP_TRACKS);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, PlaybackCommand::SetNext { next: Some(_), .. })),
        "{commands:?}"
    );
    assert_eq!(
        app.take_final_save().unwrap().mix,
        Some(SessionMix::Saved(prog.id))
    );
}

#[test]
fn a_restored_mix_needs_a_queue() {
    let now = Instant::now();
    let mut app = App::new(ROOT.into());
    app.restore_session(Session::default(), now);
    app.restore_mix(
        Some((local_mix("Prog", vec![genre("Prog Rock")]), false)),
        now,
    );
    assert_eq!(app.mix_label(), None);
}

#[test]
fn the_session_mix_is_read_again_from_the_file() {
    let mix = local_mix("Prog", vec![genre("Prog Rock")]);
    let (_dir, path) = mixes_file(&[]);
    let resumed = |m: Option<(Mix, bool)>| RestoredMix {
        mix: m,
        notice: None,
    };
    assert_eq!(
        restore_saved_mix(Some(SessionMix::Quick(mix.clone())), None),
        resumed(Some((mix.clone(), false)))
    );
    let saved = Some(SessionMix::Saved(mix.id.clone()));
    assert_eq!(
        restore_saved_mix(saved.clone(), Some(&path)),
        resumed(None),
        "gone"
    );
    store::add(&path, mix.clone()).unwrap();
    assert_eq!(
        restore_saved_mix(saved.clone(), Some(&path)),
        resumed(Some((mix, true)))
    );
    assert_eq!(restore_saved_mix(saved.clone(), None), resumed(None));
    assert_eq!(restore_saved_mix(None, Some(&path)), resumed(None));
    // A file that cannot be read is set aside, and the startup status
    // says so and where it was kept.
    std::fs::write(&path, "not json").unwrap();
    let restored = restore_saved_mix(saved, Some(&path));
    assert_eq!(restored.mix, None);
    let notice = restored.notice.expect("a notice");
    assert!(
        notice.starts_with("Could not read the mixes file: ") && notice.contains("; kept as "),
        "{notice}"
    );
    let kept = notice.split("; kept as ").nth(1).unwrap();
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "not json");
}

// ----- Navidrome -----------------------------------------------------------

fn client() -> navidrome::Client {
    navidrome::Client::new(
        parse_base_url("https://h.example/nd").unwrap(),
        "u".into(),
        Password::new("p".into()),
    )
    .unwrap()
}

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

async fn genre_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Prog Rock"))
        // A full sample, so not the whole genre: it could be asked for
        // again.
        .respond_with(ok(
            json!({ "randomSongs": { "song": songs("p", RANDOM_SONGS_SIZE) } }),
        ))
        .mount(&server)
        .await;
    server
}

fn server_client(server: &MockServer) -> navidrome::Client {
    navidrome::Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new("s3cret-pass".into()),
    )
    .unwrap()
}

/// The loop's mix arm in a test: spawn what the app wants (as the loop's
/// `drive_mixes` does) and apply the next fetch result.
struct Arm {
    fetches: Fetches,
    channels: MixChannels,
    fetched_rx: mpsc::UnboundedReceiver<MixFetched>,
}

impl Arm {
    fn new() -> Self {
        let (fetched, fetched_rx) = mpsc::unbounded_channel();
        let (loaded, _) = mpsc::unbounded_channel();
        let (saved, _) = mpsc::unbounded_channel();
        Self {
            fetches: Fetches::new(),
            channels: MixChannels {
                loaded,
                saved,
                fetched,
            },
            fetched_rx,
        }
    }

    fn drive(&self, app: &mut App) {
        drive_mixes(app, &self.fetches, &self.channels, Instant::now());
    }

    /// No fetch result arrives (a fetch spawned would answer quickly
    /// from the local mock server).
    async fn nothing_more(&mut self) {
        let next = tokio::time::timeout(Duration::from_millis(300), self.fetched_rx.recv()).await;
        assert!(next.is_err(), "unexpected fetch result: {next:?}");
    }

    async fn next(&mut self, app: &mut App) -> Vec<PlaybackCommand> {
        let (tag, fetched) = tokio::time::timeout(Duration::from_secs(10), self.fetched_rx.recv())
            .await
            .expect("a fetch result")
            .unwrap();
        let commands = app.mix_fetched(tag, fetched, Instant::now());
        self.drive(app);
        commands
    }
}

fn prog_mix(client: &navidrome::Client) -> Mix {
    Mix::new(
        "Prog",
        MixSource::Navidrome {
            server: navidrome::server_id(client),
        },
        vec![genre("Prog Rock")],
    )
    .unwrap()
}

#[tokio::test]
async fn a_navidrome_mix_loads_then_starts_and_tops_up_from_its_pool() {
    let server = genre_server().await;
    let client = server_client(&server);
    let (mut app, now) = local_app(Some(client.clone()));
    let mut arm = Arm::new();
    let commands = app.play_mix(prog_mix(&client), true, now);
    let commands = app.dispatch(commands, now);
    assert_eq!(loads(&commands), 0, "nothing plays before the songs arrive");
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog (loading…)"));
    assert_eq!(status(&app, now), "Loading mix Prog…");
    arm.drive(&mut app);
    let commands = arm.next(&mut app).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(queue_len(&app), mix::START_TRACKS);
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog"));
    // Topping up from the pool, without another request.
    play_queue_entry(&mut app, 22, now);
    assert_eq!(queue_len(&app), mix::START_TRACKS + mix::TOP_UP_TRACKS);
    assert!(app.take_mix_fetches().is_empty(), "no fetch is due");
    assert_eq!(app.mix_retry_at(), None);
    arm.drive(&mut app);
    arm.nothing_more().await;
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
async fn a_play_while_a_navidrome_mix_loads_supersedes_it() {
    let server = genre_server().await;
    let client = server_client(&server);
    let (mut app, now) = local_app(Some(client.clone()));
    let mut arm = Arm::new();
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    arm.drive(&mut app);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    // Adding songs keeps it loading.
    open_local(&mut app, LocalHomeEntry::Albums, now);
    press(&mut app, &[Action::Enqueue], now);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    // Playing an album is the newer request.
    press(&mut app, &[Action::Activate], now);
    assert_eq!(app.pending_mix_name(), None);
    assert_eq!(app.mix_label(), None);
    let before = queue_titles(&app);
    let commands = arm.next(&mut app).await;
    assert!(commands.is_empty());
    assert_eq!(queue_titles(&app), before, "the late songs change nothing");
}

#[tokio::test]
async fn another_account_ends_a_navidrome_mix_and_starts_the_marks_over() {
    let server = genre_server().await;
    let client = server_client(&server);
    let (mut app, now) = local_app(Some(client.clone()));
    let mut arm = Arm::new();
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    arm.drive(&mut app);
    arm.next(&mut app).await;
    assert_eq!(app.playing_mix_name().as_deref(), Some("Prog"));
    // Mark a Navidrome artist (the Artists view answered by hand).
    press(&mut app, &[Action::SelectNavidrome], now);
    let at = browse::HOME_ENTRIES
        .iter()
        .position(|e| *e == browse::HomeEntry::Artists)
        .unwrap();
    for _ in 0..at {
        press(&mut app, &[Action::Down], now);
    }
    press(&mut app, &[Action::Open], now);
    let request = app.take_fetch_requests().pop().unwrap();
    assert_eq!(request.resource, Resource::Artists);
    app.handle_fetch_response(
        FetchResponse {
            generation: request.generation,
            id: request.id,
            resource: request.resource,
            result: Ok(Content::Artists(vec![Artist {
                id: "ar-1".into(),
                name: "Yes".into(),
                album_count: None,
                cover_art: None,
            }])),
        },
        now,
    );
    press(&mut app, &[Action::ToggleMark], now);
    assert_eq!(
        app.marks(Source::Navidrome).unwrap().marked(),
        [Ingredient::Artist {
            name: "Yes".into(),
            id: Some("ar-1".into())
        }]
    );
    // The same account again keeps both.
    app.swap_client(Some(client.clone()), false, now);
    assert_eq!(app.playing_mix_name().as_deref(), Some("Prog"));
    assert_eq!(app.marks(Source::Navidrome).unwrap().len(), 1);
    // Another account: the mix ends, the marks start over.
    let other = navidrome::Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "someone-else".into(),
        Password::new("p".into()),
    )
    .unwrap();
    app.swap_client(Some(other), false, now);
    assert_eq!(app.mix_label(), None);
    assert_eq!(status(&app, now), OTHER_SERVER);
    assert!(app.marks(Source::Navidrome).unwrap().is_empty());
}

#[tokio::test]
async fn marked_navidrome_rows_play_as_a_navidrome_quick_mix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar-1"))
        .respond_with(ok(
            json!({ "artist": { "id": "ar-1", "name": "Yes", "album": [
                { "id": "al-1", "name": "Fragile", "artist": "Yes" }
            ]}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al-1"))
        .respond_with(ok(
            json!({ "album": { "id": "al-1", "song": songs("y", 9) }}),
        ))
        .mount(&server)
        .await;
    let client = server_client(&server);
    let now = Instant::now();
    let mut app = App::with_sources(None, Some(client.clone()));
    let mut arm = Arm::new();
    // Mark through the browser's Artists view, answered by hand.
    let at = browse::HOME_ENTRIES
        .iter()
        .position(|e| *e == browse::HomeEntry::Artists)
        .unwrap();
    for _ in 0..at {
        press(&mut app, &[Action::Down], now);
    }
    press(&mut app, &[Action::Open], now);
    let request = app.take_fetch_requests().pop().unwrap();
    app.handle_fetch_response(
        FetchResponse {
            generation: request.generation,
            id: request.id,
            resource: request.resource,
            result: Ok(Content::Artists(vec![Artist {
                id: "ar-1".into(),
                name: "Yes".into(),
                album_count: None,
                cover_art: None,
            }])),
        },
        now,
    );
    press(&mut app, &[Action::ToggleMark, Action::PlayMarked], now);
    assert_eq!(
        app.mix_label().as_deref(),
        Some("Quick mix: Yes (loading…)")
    );
    assert!(app.marks(Source::Navidrome).unwrap().is_empty());
    // Its fetch goes out through the loop's arm; it starts once it is in.
    arm.drive(&mut app);
    let commands = arm.next(&mut app).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(app.mix_label().as_deref(), Some("Quick mix: Yes"));
    assert_eq!(queue_len(&app), mix::START_TRACKS);
    assert!(
        queue_titles(&app).iter().all(|t| t.starts_with("Song")),
        "{:?}",
        queue_titles(&app)
    );
}

// ----- reading a saved mix again, superseding, the Mixes views ------------

/// Opens the Navidrome Home's `entry` (from anywhere in the browser).
fn open_remote(app: &mut App, entry: browse::HomeEntry, now: Instant) {
    press(app, &[Action::SelectNavidrome], now);
    while app
        .browser()
        .and_then(Browser::current)
        .is_some_and(|v| v.resource != Resource::Home)
    {
        press(app, &[Action::Parent], now);
    }
    let at = browse::HOME_ENTRIES
        .iter()
        .position(|e| *e == entry)
        .unwrap();
    let selected = |app: &App| app.browser().and_then(Browser::current).unwrap().selected;
    while selected(app) > at {
        press(app, &[Action::Up], now);
    }
    while selected(app) < at {
        press(app, &[Action::Down], now);
    }
    press(app, &[Action::Open], now);
}

/// Answers the browser's one outstanding fetch with `content`.
fn answer(app: &mut App, content: Content, now: Instant) {
    let request = app.take_fetch_requests().pop().expect("a fetch request");
    app.handle_fetch_response(
        FetchResponse {
            generation: request.generation,
            id: request.id,
            resource: request.resource,
            result: Ok(content),
        },
        now,
    );
}

#[test]
fn enter_on_a_saved_mix_reads_it_again_by_id() {
    let prog = local_mix("Prog", vec![genre("Prog Rock")]);
    let (_dir, path) = mixes_file(std::slice::from_ref(&prog));
    let (mut app, now) = local_app(None);
    app.set_mixes_path(Some(path.clone()));
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    read_mixes(&mut app, now);

    // Changed in the desktop app after the view read the file: the new
    // recipe plays.
    let mut changed = prog.clone();
    changed.ingredients = vec![genre("Jazz")];
    store::update(&path, changed).unwrap();
    press(&mut app, &[Action::Activate], now);
    let load = app.take_mix_load().unwrap();
    assert_eq!(load.id, prog.id);
    assert_eq!(load.path, path);
    let commands = app.mix_loaded(load.read(), now);
    assert_eq!(loads(&commands), 1);
    assert!(
        queue_titles(&app).iter().all(|t| t.starts_with("Miles")),
        "{:?}",
        queue_titles(&app)
    );
    let before = queue_titles(&app);

    // Deleted meanwhile: it says so and nothing changes.
    store::delete(&path, &prog.id, &MixSource::Local).unwrap();
    press(&mut app, &[Action::Activate], now);
    assert!(read_saved(&mut app, now).is_empty());
    assert_eq!(status(&app, now), GONE);
    assert_eq!(queue_titles(&app), before);

    // Written by a newer Coda: the store's message.
    std::fs::write(&path, r#"{"version":99,"mixes":[]}"#).unwrap();
    press(&mut app, &[Action::Activate], now);
    assert!(read_saved(&mut app, now).is_empty());
    assert_eq!(
        status(&app, now),
        "The mixes file was written by a newer version of Coda"
    );

    // Another play request before the read is back: it does not start.
    std::fs::remove_file(&path).unwrap();
    store::add(&path, prog.clone()).unwrap();
    press(&mut app, &[Action::Activate], now);
    let load = app.take_mix_load().unwrap();
    let mut late = load.clone();
    open_local(&mut app, LocalHomeEntry::Albums, now);
    press(&mut app, &[Action::Activate], now);
    let album = queue_titles(&app);
    assert!(app.mix_loaded(load.read(), now).is_empty());
    assert_eq!(queue_titles(&app), album);
    // The queue replaced meanwhile (same request, another context): it
    // does not start either.
    late.serial = app.mix_request;
    assert!(app.mix_loaded(late.read(), now).is_empty());
    assert_eq!(queue_titles(&app), album);
    assert_eq!(app.mix_label(), None);
}

#[test]
fn a_rejected_mixes_file_is_told_and_the_view_reads_again() {
    let prog = local_mix("Prog", vec![genre("Prog Rock")]);
    let (_dir, path) = mixes_file(std::slice::from_ref(&prog));
    let (mut app, now) = local_app(None);
    app.set_mixes_path(Some(path.clone()));
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    read_mixes(&mut app, now);
    std::fs::write(&path, "not json").unwrap();
    press(&mut app, &[Action::Activate], now);
    assert!(read_saved(&mut app, now).is_empty());
    assert!(
        status(&app, now).starts_with("Could not read the mixes file:"),
        "{}",
        status(&app, now)
    );
    assert!(app.take_mixes_read().is_some());
}

#[test]
fn a_newer_mixes_file_is_told_and_the_view_reads_again() {
    let prog = local_mix("Prog", vec![genre("Prog Rock")]);
    let (_dir, path) = mixes_file(std::slice::from_ref(&prog));
    let (mut app, now) = local_app(None);
    app.set_mixes_path(Some(path.clone()));
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    read_mixes(&mut app, now);
    std::fs::write(&path, r#"{"version":99,"mixes":[]}"#).unwrap();
    press(&mut app, &[Action::Activate], now);
    assert!(read_saved(&mut app, now).is_empty());
    assert_eq!(
        status(&app, now),
        "The mixes file was written by a newer version of Coda"
    );
    assert!(app.take_mixes_read().is_some());
}

#[test]
fn every_valid_play_supersedes_a_loading_mix_even_when_it_finds_nothing() {
    let client = client();
    // No songs scanned yet.
    let now = Instant::now();
    let mut app = App::with_sources(Some(ROOT.into()), Some(client.clone()));
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    // Enter that opens a row is no play request.
    open_local(&mut app, LocalHomeEntry::Genres, now);
    press(&mut app, &[Action::Parent], now);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    // Shuffle all finds nothing, but it was asked for.
    while app.library_selected() > 0 {
        press(&mut app, &[Action::Up], now);
    }
    press(&mut app, &[Action::Activate], now);
    assert_eq!(
        status(&app, now),
        "Nothing to play (0 tracks scanned so far)"
    );
    assert_eq!(app.pending_mix_name(), None);
    assert_eq!(app.mix_label(), None);

    // A Navidrome playlist cached empty: Enter plays nothing at once.
    open_remote(&mut app, browse::HomeEntry::Playlists, now);
    answer(
        &mut app,
        Content::Playlists(vec![Playlist {
            id: "pl-1".into(),
            name: "Empty".into(),
            song_count: Some(0),
            duration: None,
            access: Access::default(),
            cover_art: None,
        }]),
        now,
    );
    press(&mut app, &[Action::Open], now);
    answer(
        &mut app,
        Content::PlaylistSongs(PlaylistSongs::default()),
        now,
    );
    press(&mut app, &[Action::Parent], now);
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    press(&mut app, &[Action::Activate], now);
    assert!(app.take_fetch_requests().is_empty(), "cached");
    assert_eq!(app.pending_mix_name(), None);
    assert_eq!(app.mix_label(), None);
}

#[test]
fn a_refused_mix_leaves_the_loading_one() {
    let client = client();
    let now = Instant::now();
    // No local library.
    let mut app = App::with_sources(None, Some(client.clone()));
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    assert!(
        app.play_mix(local_mix("Jazz", vec![genre("Jazz")]), false, now)
            .is_empty()
    );
    assert_eq!(status(&app, now), NO_LOCAL_LIBRARY);
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    // Made on another server.
    let elsewhere = Mix::new(
        "Elsewhere",
        MixSource::Navidrome {
            server: navidrome::server_id_of("https://other.example/", "u"),
        },
        vec![genre("Jazz")],
    )
    .unwrap();
    assert!(app.play_mix(elsewhere, true, now).is_empty());
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
}

#[test]
fn switching_between_two_mixes_views_reads_the_file_again() {
    let (_dir, path) = mixes_file(&[local_mix("Prog", vec![genre("Prog Rock")])]);
    let (mut app, now) = local_app(Some(client()));
    app.set_mixes_path(Some(path.clone()));
    open_local(&mut app, LocalHomeEntry::Mixes, now);
    read_mixes(&mut app, now);
    open_remote(&mut app, browse::HomeEntry::Mixes, now);
    read_mixes(&mut app, now);
    // Both browsers now show their Mixes view. A mix added elsewhere
    // shows when the other source's view is shown again.
    store::add(&path, local_mix("Jazz", vec![genre("Jazz")])).unwrap();
    press(&mut app, &[Action::SelectLocal], now);
    read_mixes(&mut app, now);
    assert_eq!(app.local_browser().mixes().len(), 2);
    press(&mut app, &[Action::SelectNavidrome], now);
    assert!(app.take_mixes_read().is_some());
    // The focus moving away and back reads nothing.
    press(&mut app, &[Action::SwitchFocus, Action::SwitchFocus], now);
    assert!(app.take_mixes_read().is_none());
}

#[test]
fn marks_say_where_they_work() {
    let (mut app, now) = local_app(None);
    open_local(&mut app, LocalHomeEntry::Genres, now);
    // Genres A–Z: Jazz first.
    press(&mut app, &[Action::ToggleMark], now);
    // Local search results are songs.
    press(&mut app, &[Action::OpenSearch], now);
    for c in "Miles".chars() {
        press(&mut app, &[Action::InputChar(c)], now);
    }
    press(&mut app, &[Action::InputSubmit], now);
    press(&mut app, &[Action::ToggleMark], now);
    assert_eq!(status(&app, now), MARKS_IN_LISTS);
    assert_eq!(app.marks(Source::Local).unwrap().len(), 1);
    // `M` still plays the marks from there.
    let commands = press(&mut app, &[Action::PlayMarked], now);
    assert_eq!(loads(&commands), 1);
    assert_eq!(app.mix_label().as_deref(), Some("Quick mix: Jazz"));
}

#[test]
fn a_navidrome_album_without_an_artist_cannot_be_marked() {
    let (mut app, now) = local_app(Some(client()));
    open_remote(&mut app, browse::HomeEntry::Albums, now);
    let album = |id: &str, artist: Option<&str>| Album {
        id: id.into(),
        name: format!("Album {id}"),
        artist: artist.map(Into::into),
        year: None,
        song_count: None,
        duration: None,
        cover_art: None,
        artist_id: None,
    };
    answer(
        &mut app,
        Content::AlbumPage(AlbumPage {
            albums: vec![album("al-1", None), album("al-2", Some("Yes"))],
            served: 2,
        }),
        now,
    );
    press(&mut app, &[Action::ToggleMark], now);
    assert_eq!(status(&app, now), NO_ALBUM_ARTIST);
    press(&mut app, &[Action::Down, Action::ToggleMark], now);
    assert_eq!(app.marks(Source::Navidrome).unwrap().len(), 1);
}

// ----- the loop's retry timer and errors -----------------------------------

#[tokio::test]
async fn a_failed_fetch_is_retried_on_the_timer_and_the_mix_then_starts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Prog Rock"))
        .respond_with(ok(json!({ "randomSongs": { "song": songs("p", 60) } })))
        .mount(&server)
        .await;
    let client = server_client(&server);
    let (mut app, now) = local_app(Some(client.clone()));
    let mut arm = Arm::new();
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    arm.drive(&mut app);
    let before = Instant::now();
    assert!(arm.next(&mut app).await.is_empty());
    let after = Instant::now();
    // Still loading, with a retry armed and nothing due before it.
    assert_eq!(app.pending_mix_name().as_deref(), Some("Prog"));
    let retry = app.mix_retry_at().expect("a retry");
    assert!(
        retry >= before + RETRY_AFTER && retry <= after + RETRY_AFTER,
        "{retry:?}"
    );
    assert!(app.mix_retry(retry - Duration::from_secs(1)).is_empty());
    assert!(app.take_mix_fetches().is_empty());
    // The timer fires: the fetch goes out again, and the mix starts.
    app.mix_retry(retry);
    arm.drive(&mut app);
    let commands = arm.next(&mut app).await;
    assert_eq!(loads(&commands), 1);
    assert_eq!(app.playing_mix_name().as_deref(), Some("Prog"));
    assert_eq!(app.mix_retry_at(), None);
}

#[test]
fn fetches_without_a_client_end_the_navidrome_mixes() {
    let client = client();
    let (mut app, now) = local_app(Some(client.clone()));
    // A mix loading.
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    app.mix_fetches_unsent(now);
    assert_eq!(app.mix_label(), None);
    assert_eq!(status(&app, now), NO_SERVER);
    // A mix playing (restored with the session).
    let session = Session {
        tracks: vec![track(0, "Yes", "Prog Rock")],
        current: Some(0),
        ..Session::default()
    };
    app.restore_session(session, now);
    app.restore_mix(Some((prog_mix(&client), true)), now);
    assert_eq!(app.mix_label().as_deref(), Some("Mix: Prog"));
    app.mix_fetches_unsent(now);
    assert_eq!(app.mix_label(), None);
    // A local mix is not affected.
    let commands = app.play_mix(local_mix("Jazz", vec![genre("Jazz")]), false, now);
    app.dispatch(commands, now);
    app.mix_fetches_unsent(now);
    assert_eq!(app.mix_label().as_deref(), Some("Quick mix: Jazz"));
}

/// Spawns the due fetches with `client` (another account than the
/// mix's), as the loop would after a swap it has not seen yet.
fn spawn_with(app: &mut App, arm: &Arm, client: &navidrome::Client) -> usize {
    let due = app.take_mix_fetches();
    let n = due.len();
    for (tag, fetch) in due {
        let tx = arm.channels.fetched.clone();
        drop(
            fetch.spawn(client.clone(), arm.fetches.limit.clone(), move |result| {
                let _ = tx.send((tag, result));
            }),
        );
    }
    n
}

#[tokio::test]
async fn a_fetch_answered_for_another_server_ends_the_mix() {
    let server = genre_server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Jazz"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let client = server_client(&server);
    let other = navidrome::Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "someone-else".into(),
        Password::new("p".into()),
    )
    .unwrap();
    let (mut app, now) = local_app(Some(client.clone()));
    let mut arm = Arm::new();

    // Loading: the mix does not start.
    let commands = app.play_mix(prog_mix(&client), true, now);
    app.dispatch(commands, now);
    assert_eq!(spawn_with(&mut app, &arm, &other), 1);
    assert!(arm.next(&mut app).await.is_empty());
    assert_eq!(app.mix_label(), None);
    assert_eq!(status(&app, now), OTHER_SERVER);

    // Playing: Prog Rock arrived, Jazz failed and is asked for again.
    let both = Mix::new(
        "Both",
        MixSource::Navidrome {
            server: navidrome::server_id(&client),
        },
        vec![genre("Prog Rock"), genre("Jazz")],
    )
    .unwrap();
    let commands = app.play_mix(both, true, now);
    app.dispatch(commands, now);
    arm.drive(&mut app);
    while app.playing_mix_name().is_none() {
        arm.next(&mut app).await;
    }
    assert!(
        status(&app, now).contains("could not load genre Jazz, trying again later"),
        "{}",
        status(&app, now)
    );
    let retry = app.mix_retry_at().expect("Jazz again");
    app.mix_retry(retry);
    assert_eq!(spawn_with(&mut app, &arm, &other), 1);
    arm.next(&mut app).await;
    assert_eq!(app.mix_label(), None);
    assert_eq!(status(&app, now), OTHER_SERVER);
    let asked = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.query().is_some_and(|q| q.contains("someone-else")))
        .count();
    assert_eq!(asked, 0, "never asked with the other account");
}
