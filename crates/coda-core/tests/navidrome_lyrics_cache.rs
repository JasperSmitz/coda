//! The Navidrome lyrics cache (`coda_core::lyrics::cache`) synced against
//! a local `wiremock` server that plays a small Navidrome library. No test
//! contacts a real server.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coda_core::lyrics::cache::{
    self, Pause, State, Store, SyncError, SyncOptions, SyncReport, file_path, server_key,
};
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const USERNAME: &str = "listener";
const PASSWORD: &str = "s3cret-pass";
const DAY: u64 = 24 * 60 * 60;
const T0: u64 = 1_800_000_000;

#[derive(Clone)]
enum Lyr {
    Timed(Vec<(u64, &'static str)>),
    Plain(Vec<&'static str>),
    None,
    /// HTTP 500.
    Down,
    /// Subsonic error 40 (wrong credentials).
    Rejected,
    /// This HTTP status.
    Status(u16),
    /// A 200 HTML page (a proxy's login or error page).
    Html,
}

#[derive(Clone)]
struct Song {
    id: String,
    title: String,
    lyrics: Lyr,
}

#[derive(Clone)]
struct Album {
    id: String,
    name: String,
    artist: String,
    songs: Vec<Song>,
}

/// The fake server's library and what it was asked.
#[derive(Default)]
struct World {
    albums: Vec<Album>,
    /// `getOpenSubsonicExtensions` answers with an HTML page.
    extensions_html: bool,
    /// `getAlbumList2` lists only the first this many albums.
    list_only: Option<usize>,
    /// (endpoint, id or offset, when).
    log: Vec<(String, String, Instant)>,
}

#[derive(Clone)]
struct Fake {
    world: Arc<Mutex<World>>,
    delay: Duration,
}

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1", "type": "navidrome" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn failed(code: i64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": code, "message": "no" }
        }
    }))
}

fn html() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw("<html><body>Sign in</body></html>", "text/html")
}

fn param(request: &Request, key: &str) -> String {
    request
        .url
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

impl Respond for Fake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let endpoint = request.url.path().trim_start_matches("/rest/").to_owned();
        let mut world = self.world.lock().unwrap();
        let key = match endpoint.as_str() {
            "getAlbumList2" => param(request, "offset"),
            _ => param(request, "id"),
        };
        world
            .log
            .push((endpoint.clone(), key.clone(), Instant::now()));
        let response = match endpoint.as_str() {
            "getOpenSubsonicExtensions" if world.extensions_html => html(),
            "getOpenSubsonicExtensions" => ok(json!({ "openSubsonicExtensions": [
                { "name": "songLyrics", "versions": [1] }
            ]})),
            "getAlbumList2" => {
                assert_eq!(param(request, "type"), "alphabeticalByName");
                let offset: usize = key.parse().unwrap();
                let size: usize = param(request, "size").parse().unwrap();
                let page: Vec<Value> = world
                    .albums
                    .iter()
                    .take(world.list_only.unwrap_or(usize::MAX))
                    .skip(offset)
                    .take(size)
                    .map(|a| {
                        json!({ "id": a.id, "name": a.name, "artist": a.artist,
                                "songCount": a.songs.len() })
                    })
                    .collect();
                ok(json!({ "albumList2": { "album": page } }))
            }
            "getAlbum" => match world.albums.iter().find(|a| a.id == key) {
                Some(a) => {
                    let songs: Vec<Value> = a
                        .songs
                        .iter()
                        .enumerate()
                        .map(|(n, s)| {
                            json!({ "id": s.id, "title": s.title, "artist": a.artist,
                                    "album": a.name, "albumId": a.id, "track": n + 1,
                                    "discNumber": 1, "duration": 180,
                                    "coverArt": format!("mf-{}", s.id) })
                        })
                        .collect();
                    ok(json!({ "album": { "id": a.id, "name": a.name,
                                          "songCount": a.songs.len(), "song": songs } }))
                }
                None => failed(70),
            },
            "getLyricsBySongId" => {
                let song = world
                    .albums
                    .iter()
                    .flat_map(|a| &a.songs)
                    .find(|s| s.id == key)
                    .map(|s| s.lyrics.clone());
                match song {
                    Some(Lyr::Timed(lines)) => {
                        let lines: Vec<Value> = lines
                            .iter()
                            .map(|(ms, text)| json!({ "start": ms, "value": text }))
                            .collect();
                        ok(json!({ "lyricsList": { "structuredLyrics": [
                            { "lang": "eng", "synced": true, "line": lines }
                        ]}}))
                    }
                    Some(Lyr::Plain(lines)) => {
                        let lines: Vec<Value> =
                            lines.iter().map(|text| json!({ "value": text })).collect();
                        ok(json!({ "lyricsList": { "structuredLyrics": [
                            { "lang": "eng", "synced": false, "line": lines }
                        ]}}))
                    }
                    Some(Lyr::None) => ok(json!({ "lyricsList": { "structuredLyrics": [] } })),
                    Some(Lyr::Down) => ResponseTemplate::new(500),
                    Some(Lyr::Rejected) => failed(40),
                    Some(Lyr::Status(status)) => ResponseTemplate::new(status),
                    Some(Lyr::Html) => html(),
                    None => failed(70),
                }
            }
            other => panic!("unexpected request to {other}"),
        };
        response.set_delay(self.delay)
    }
}

struct Server {
    _server: MockServer,
    client: Client,
    world: Arc<Mutex<World>>,
    uri: String,
}

impl Server {
    async fn start(albums: Vec<Album>, delay: Duration) -> Self {
        let server = MockServer::start().await;
        let world = Arc::new(Mutex::new(World {
            albums,
            ..World::default()
        }));
        Mock::given(wiremock::matchers::any())
            .respond_with(Fake {
                world: world.clone(),
                delay,
            })
            .mount(&server)
            .await;
        let client = Client::new(
            parse_base_url(&server.uri()).unwrap(),
            USERNAME.into(),
            Password::new(PASSWORD.into()),
        )
        .unwrap();
        Self {
            uri: server.uri(),
            _server: server,
            client,
            world,
        }
    }

    /// The requests to `endpoint` since the last call, by id (or offset),
    /// and clears the log.
    fn asked(&self, endpoint: &str) -> Vec<String> {
        let world = self.world.lock().unwrap();
        let mut ids: Vec<String> = world
            .log
            .iter()
            .filter(|(e, _, _)| e == endpoint)
            .map(|(_, id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn clear(&self) {
        self.world.lock().unwrap().log.clear();
    }

    fn edit(&self, f: impl FnOnce(&mut Vec<Album>)) {
        f(&mut self.world.lock().unwrap().albums);
    }
}

fn song(id: &str, title: &str, lyrics: Lyr) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        lyrics,
    }
}

fn album(id: &str, artist: &str, songs: Vec<Song>) -> Album {
    Album {
        id: id.into(),
        name: format!("Album {id}"),
        artist: artist.into(),
        songs,
    }
}

/// Two albums: timed, untimed and no lyrics.
fn library() -> Vec<Album> {
    vec![
        album(
            "a1",
            "Band",
            vec![
                song(
                    "s1",
                    "River",
                    Lyr::Timed(vec![(1000, "Golden river"), (5000, "runs to the sea")]),
                ),
                song("s2", "Quiet", Lyr::None),
            ],
        ),
        album(
            "a2",
            "Act",
            vec![song(
                "s3",
                "Hills",
                Lyr::Plain(vec!["Over the hills", "and far away"]),
            )],
        ),
    ]
}

fn options(now: u64) -> SyncOptions {
    SyncOptions {
        now,
        ..SyncOptions::new(false)
    }
}

fn titles(store: &Store, query: &str) -> Vec<String> {
    store
        .search(query, &HashSet::new(), 50)
        .matches
        .into_iter()
        .map(|m| m.key.title)
        .collect()
}

#[tokio::test]
async fn a_first_sync_indexes_timed_and_untimed_lyrics() {
    let server = Server::start(library(), Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let report = cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    assert_eq!(
        report,
        SyncReport {
            albums: 2,
            songs: 3,
            left: 0,
            skipped: 0,
            attempted: 5,
        }
    );
    let status = store.status();
    assert_eq!(status.state, State::UpToDate);
    assert_eq!(
        (status.songs, status.checked, status.with_lyrics),
        (3, 3, 2)
    );
    // A phrase across two timed lines; timestamps never show.
    let found = store.search("river runs", &HashSet::new(), 50);
    assert_eq!(found.total, 1);
    let m = &found.matches[0];
    assert_eq!(m.snippet.text, "Golden river / runs to the sea");
    assert_eq!(m.key.navidrome_id(), Some("s1"));
    assert_eq!(m.key.album_id.as_deref(), Some("a1"));
    assert_eq!(m.key.album.as_deref(), Some("Album a1"));
    assert_eq!(m.key.cover_art.as_deref(), Some("mf-s1"));
    assert_eq!(titles(&store, "far away"), ["Hills"]);
    assert_eq!(server.asked("getAlbum"), ["a1", "a2"]);
    assert_eq!(server.asked("getLyricsBySongId"), ["s1", "s2", "s3"]);
}

#[tokio::test]
async fn albums_are_listed_over_several_pages() {
    let albums: Vec<Album> = (0..1001)
        .map(|n| {
            album(
                &format!("al{n:04}"),
                "Many",
                vec![song(&format!("so{n:04}"), "Song", Lyr::None)],
            )
        })
        .collect();
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let report = cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    assert_eq!(report.albums, 1001);
    assert_eq!(report.songs, 1001);
    assert_eq!(server.asked("getAlbumList2"), ["0", "1000", "500"]);
    assert_eq!(store.status().songs, 1001);
}

#[tokio::test]
async fn a_new_album_fetches_only_that_album_and_its_songs() {
    let server = Server::start(library(), Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    server.clear();
    server.edit(|albums| {
        albums.push(album(
            "a3",
            "Choir",
            vec![song("s4", "Bells", Lyr::Plain(vec!["ring the bells"]))],
        ))
    });
    let report = cache::sync(&store, &server.client, options(T0 + DAY))
        .await
        .unwrap();
    assert_eq!((report.albums, report.songs), (1, 1));
    assert_eq!(server.asked("getAlbum"), ["a3"]);
    assert_eq!(server.asked("getLyricsBySongId"), ["s4"]);
    assert_eq!(titles(&store, "the bells"), ["Bells"]);
    // Nothing changed: only the album list is asked.
    server.clear();
    let report = cache::sync(&store, &server.client, options(T0 + 2 * DAY))
        .await
        .unwrap();
    assert_eq!(report, SyncReport::default());
    assert!(server.asked("getAlbum").is_empty());
    assert_eq!(server.asked("getAlbumList2"), ["0"]);
}

#[tokio::test]
async fn a_song_count_change_refetches_only_that_album() {
    let server = Server::start(library(), Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    server.clear();
    server.edit(|albums| {
        albums[0]
            .songs
            .push(song("s5", "Encore", Lyr::Plain(vec!["one more time"])))
    });
    cache::sync(&store, &server.client, options(T0 + DAY))
        .await
        .unwrap();
    assert_eq!(server.asked("getAlbum"), ["a1"]);
    // The known songs keep their lyrics without being asked again.
    assert_eq!(server.asked("getLyricsBySongId"), ["s5"]);
    assert_eq!(titles(&store, "golden river"), ["River"]);
    assert_eq!(titles(&store, "one more time"), ["Encore"]);
}

#[tokio::test]
async fn a_removed_album_is_dropped() {
    let server = Server::start(library(), Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    server.edit(|albums| {
        albums.remove(0);
    });
    cache::sync(&store, &server.client, options(T0 + DAY))
        .await
        .unwrap();
    assert!(titles(&store, "golden river").is_empty());
    assert_eq!(titles(&store, "far away"), ["Hills"]);
    let status = store.status();
    assert_eq!(
        (status.songs, status.checked, status.with_lyrics),
        (1, 1, 1)
    );
}

#[tokio::test]
async fn a_transient_error_is_retried_next_time() {
    let mut albums = library();
    albums[1].songs[0].lyrics = Lyr::Down;
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let report = cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    assert_eq!(report.left, 1);
    assert_eq!(store.status().state, State::Paused(Pause::Unreachable));
    assert_eq!(store.status().checked, 2);
    server.clear();
    server.edit(|albums| albums[1].songs[0].lyrics = Lyr::Plain(vec!["back again"]));
    let report = cache::sync(&store, &server.client, options(T0 + 60))
        .await
        .unwrap();
    assert_eq!((report.songs, report.left), (1, 0));
    assert_eq!(server.asked("getLyricsBySongId"), ["s3"]);
    assert_eq!(store.status().state, State::UpToDate);
    assert_eq!(titles(&store, "back again"), ["Hills"]);
}

#[tokio::test]
async fn rejected_credentials_stop_the_sync() {
    let mut albums = library();
    for song in &mut albums[0].songs {
        song.lyrics = Lyr::Rejected;
    }
    albums[1].songs[0].lyrics = Lyr::Rejected;
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let result = cache::sync(&store, &server.client, options(T0)).await;
    assert_eq!(result, Err(SyncError::Rejected));
    assert_eq!(store.status().state, State::Paused(Pause::Rejected));
    assert_eq!(store.status().checked, 0);
    // At most the requests already in flight were sent.
    assert!(server.asked("getLyricsBySongId").len() <= cache::MAX_IN_FLIGHT);
}

#[tokio::test]
async fn an_unreachable_server_pauses_without_dropping_anything() {
    let server = Server::start(library(), Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let key = server_key(&server.uri, USERNAME);
    let store = Arc::new(Store::open(dir.path(), &key));
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    // A client pointing at a closed port.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let gone = Client::new(
        parse_base_url(&format!("http://{closed}")).unwrap(),
        USERNAME.into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    let result = cache::sync(&store, &gone, options(T0 + DAY)).await;
    assert_eq!(result, Err(SyncError::Unreachable));
    assert_eq!(store.status().state, State::Paused(Pause::Unreachable));
    assert_eq!(titles(&store, "golden river"), ["River"]);
    assert_eq!(Store::open(dir.path(), &key).status().songs, 3);
}

#[tokio::test]
async fn an_interrupted_build_resumes_where_it_stopped() {
    let albums: Vec<Album> = (0..4)
        .map(|n| {
            album(
                &format!("al{n}"),
                "Band",
                (0..5)
                    .map(|m| song(&format!("s{n}{m}"), "Song", Lyr::Plain(vec!["la la la"])))
                    .collect(),
            )
        })
        .collect();
    let server = Server::start(albums, Duration::from_millis(30)).await;
    let dir = tempfile::tempdir().unwrap();
    let key = server_key(&server.uri, USERNAME);
    let store = Arc::new(Store::open(dir.path(), &key));
    let frequent = SyncOptions {
        save_every: 1,
        save_interval: Duration::ZERO,
        ..options(T0)
    };
    let mut status = store.subscribe();
    let task = tokio::spawn({
        let (store, client) = (store.clone(), server.client.clone());
        async move { cache::sync(&store, &client, frequent).await }
    });
    // Stop it partway through the lyrics.
    tokio::time::timeout(Duration::from_secs(10), status.wait_for(|s| s.checked >= 8))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    // Saves run on blocking threads; let one in progress land.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let reopened = Arc::new(Store::open(dir.path(), &key));
    let saved = reopened.status().checked;
    assert!((7..20).contains(&saved), "saved {saved}");
    assert_eq!(reopened.status().songs, 20);
    server.clear();
    let report = cache::sync(&reopened, &server.client, frequent)
        .await
        .unwrap();
    assert!(
        server.asked("getAlbum").is_empty(),
        "albums not fetched again"
    );
    assert_eq!(server.asked("getLyricsBySongId").len(), 20 - saved);
    assert_eq!(report.songs, 20 - saved);
    assert_eq!(reopened.status().checked, 20);
    assert_eq!(reopened.status().state, State::UpToDate);
}

#[tokio::test]
async fn every_thirty_days_and_on_rebuild_everything_is_checked_again() {
    let server = Server::start(library(), Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    server.clear();
    cache::sync(&store, &server.client, options(T0 + 29 * DAY))
        .await
        .unwrap();
    assert!(server.asked("getLyricsBySongId").is_empty());

    // Changed lyrics with an unchanged song count show at the refresh.
    server.edit(|albums| albums[0].songs[0].lyrics = Lyr::Plain(vec!["silver lake"]));
    let report = cache::sync(&store, &server.client, options(T0 + 31 * DAY))
        .await
        .unwrap();
    assert_eq!((report.albums, report.songs), (2, 3));
    assert_eq!(titles(&store, "silver lake"), ["River"]);
    assert!(titles(&store, "golden river").is_empty());

    // The refresh is done: the next day asks nothing.
    server.clear();
    cache::sync(&store, &server.client, options(T0 + 32 * DAY))
        .await
        .unwrap();
    assert!(server.asked("getLyricsBySongId").is_empty());

    // Rebuild.
    let report = cache::sync(
        &store,
        &server.client,
        SyncOptions {
            rebuild: true,
            ..options(T0 + 33 * DAY)
        },
    )
    .await
    .unwrap();
    assert_eq!((report.albums, report.songs), (2, 3));
}

#[tokio::test]
async fn the_file_holds_no_url_or_credentials() {
    let server = Server::start(library(), Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let key = server_key(&server.uri, USERNAME);
    let store = Arc::new(Store::open(dir.path(), &key));
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    let path = file_path(dir.path(), &key);
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.is_empty());
    let host = server.uri.trim_start_matches("http://");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    for needle in [
        server.uri.as_str(),
        host,
        "127.0.0.1",
        "http",
        USERNAME,
        PASSWORD,
        "rest/",
    ] {
        let found = bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
        assert!(!found, "{needle} in the file");
        assert!(!name.contains(needle), "{needle} in the name");
    }
}

#[tokio::test]
async fn at_most_three_requests_are_in_flight() {
    let albums: Vec<Album> = (0..6)
        .map(|n| {
            album(
                &format!("al{n}"),
                "Band",
                (0..3)
                    .map(|m| song(&format!("s{n}{m}"), "Song", Lyr::None))
                    .collect(),
            )
        })
        .collect();
    let delay = Duration::from_millis(80);
    let server = Server::start(albums, delay).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    let world = server.world.lock().unwrap();
    let mut by_endpoint: HashMap<&str, Vec<Instant>> = HashMap::new();
    for (endpoint, _, at) in &world.log {
        by_endpoint.entry(endpoint).or_default().push(*at);
    }
    // A request is in flight from its arrival until its delayed answer;
    // a new one only starts once an answer is in.
    let window = delay - Duration::from_millis(10);
    for endpoint in ["getAlbum", "getLyricsBySongId"] {
        let arrivals = &by_endpoint[endpoint];
        let most = arrivals
            .iter()
            .map(|t| {
                arrivals
                    .iter()
                    .filter(|u| **u >= *t && **u < *t + window)
                    .count()
            })
            .max()
            .unwrap();
        assert_eq!(most, cache::MAX_IN_FLIGHT, "{endpoint}");
    }
}

/// One album of `n` songs with plain lyrics, ids `s0`…
fn big_album(n: usize) -> Album {
    album(
        "big",
        "Band",
        (0..n)
            .map(|m| {
                song(
                    &format!("s{m:02}"),
                    &format!("Song {m:02}"),
                    Lyr::Plain(vec!["la la la"]),
                )
            })
            .collect(),
    )
}

#[tokio::test]
async fn http_401_or_403_stops_the_sync_and_records_nothing_after_it() {
    for status in [401, 403] {
        let mut albums = vec![big_album(12)];
        for song in &mut albums[0].songs[6..] {
            song.lyrics = Lyr::Status(status);
        }
        let server = Server::start(albums, Duration::ZERO).await;
        let store = Arc::new(Store::in_memory());
        let result = cache::sync(&store, &server.client, options(T0)).await;
        assert_eq!(result, Err(SyncError::Rejected), "{status}");
        assert_eq!(store.status().state, State::Paused(Pause::Rejected));
        assert!(store.status().checked <= 6, "{status}");
        // The songs after it are asked again next time.
        server.edit(|albums| {
            for song in &mut albums[0].songs {
                song.lyrics = Lyr::Plain(vec!["la la la"]);
            }
        });
        server.clear();
        cache::sync(&store, &server.client, options(T0 + 60))
            .await
            .unwrap();
        assert_eq!(store.status().checked, 12);
        assert_eq!(store.status().state, State::UpToDate);
    }
}

#[tokio::test]
async fn html_pages_trip_the_breaker_without_recording_no_lyrics() {
    let mut albums = vec![big_album(30)];
    for song in &mut albums[0].songs[5..] {
        song.lyrics = Lyr::Html;
    }
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let result = cache::sync(&store, &server.client, options(T0)).await;
    assert_eq!(result, Err(SyncError::Failed));
    assert_eq!(store.status().state, State::Paused(Pause::Failed));
    assert_eq!(store.status().checked, 5);
    // It stopped soon after the breaker's count (plus those in flight).
    let asked = server.asked("getLyricsBySongId").len();
    assert!(
        asked <= 5 + cache::BREAKER + cache::MAX_IN_FLIGHT,
        "{asked}"
    );
    // Nothing was counted against the songs: three more such syncs do not
    // skip them.
    for n in 1..=3 {
        let _ = cache::sync(&store, &server.client, options(T0 + n)).await;
    }
    assert_eq!(store.status().checked, 5);
}

#[tokio::test]
async fn a_server_down_mid_sync_trips_the_breaker_as_unreachable() {
    let mut albums = vec![big_album(30)];
    for song in &mut albums[0].songs[5..] {
        song.lyrics = Lyr::Down;
    }
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let result = cache::sync(&store, &server.client, options(T0)).await;
    assert_eq!(result, Err(SyncError::Unreachable));
    assert_eq!(store.status().state, State::Paused(Pause::Unreachable));
    assert_eq!(store.status().checked, 5);
}

#[tokio::test]
async fn a_failing_extension_check_stops_the_sync_without_recording_anything() {
    let server = Server::start(vec![big_album(15)], Duration::ZERO).await;
    server.world.lock().unwrap().extensions_html = true;
    let store = Arc::new(Store::in_memory());
    let result = cache::sync(&store, &server.client, options(T0)).await;
    assert_eq!(result, Err(SyncError::Failed));
    assert_eq!(store.status().checked, 0);
    assert_eq!(store.status().songs, 15);
}

#[tokio::test]
async fn one_song_that_always_fails_is_skipped_after_three_syncs() {
    let mut albums = vec![big_album(40)];
    albums[0].songs[7].lyrics = Lyr::Down;
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    let report = cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    assert_eq!((report.left, report.skipped), (1, 0));
    // One failure in 41 requests is not "the server is unreachable".
    assert_eq!(store.status().state, State::UpToDate);
    assert_eq!(store.status().checked, 39);
    let report = cache::sync(&store, &server.client, options(T0 + 1))
        .await
        .unwrap();
    assert_eq!((report.left, report.skipped, report.attempted), (1, 0, 1));
    let report = cache::sync(&store, &server.client, options(T0 + 2))
        .await
        .unwrap();
    assert_eq!((report.left, report.skipped), (0, 1));
    assert_eq!(store.status().checked, 40);
    // Skipped until the next refresh; the first build's refresh is done.
    server.clear();
    let report = cache::sync(&store, &server.client, options(T0 + 3))
        .await
        .unwrap();
    assert_eq!(report.attempted, 0);
    assert!(server.asked("getLyricsBySongId").is_empty());
    // The 30-day refresh asks it again.
    server.clear();
    cache::sync(&store, &server.client, options(T0 + 31 * DAY))
        .await
        .unwrap();
    assert!(
        server
            .asked("getLyricsBySongId")
            .contains(&"s07".to_owned())
    );
}

#[tokio::test]
async fn an_empty_or_much_shorter_album_list_removes_nothing() {
    let albums: Vec<Album> = (0..30)
        .map(|n| {
            album(
                &format!("al{n:02}"),
                "Band",
                vec![song(
                    &format!("s{n:02}"),
                    "Song",
                    Lyr::Plain(vec!["la la la"]),
                )],
            )
        })
        .collect();
    let server = Server::start(albums, Duration::ZERO).await;
    let store = Arc::new(Store::in_memory());
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    assert_eq!(store.status().songs, 30);
    for shown in [0, 10] {
        server.world.lock().unwrap().list_only = Some(shown);
        let result = cache::sync(&store, &server.client, options(T0 + DAY)).await;
        assert_eq!(result, Err(SyncError::Failed), "{shown}");
        assert_eq!(store.status().state, State::Paused(Pause::Failed));
        assert_eq!(store.status().songs, 30, "{shown}");
    }
    // A moderate shrink is a library change.
    server.world.lock().unwrap().list_only = Some(20);
    cache::sync(&store, &server.client, options(T0 + DAY))
        .await
        .unwrap();
    assert_eq!(store.status().songs, 20);
    // Rebuild accepts a sharp one.
    server.world.lock().unwrap().list_only = Some(2);
    let rebuild = SyncOptions {
        rebuild: true,
        ..options(T0 + 2 * DAY)
    };
    cache::sync(&store, &server.client, rebuild).await.unwrap();
    assert_eq!(store.status().songs, 2);
}

#[tokio::test]
async fn an_unchanged_sync_does_not_rewrite_the_file() {
    let server = Server::start(library(), Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let key = server_key(&server.uri, USERNAME);
    let store = Arc::new(Store::open(dir.path(), &key));
    cache::sync(&store, &server.client, options(T0))
        .await
        .unwrap();
    let path = file_path(dir.path(), &key);
    let reopened = Arc::new(Store::open(dir.path(), &key));
    std::fs::remove_file(&path).unwrap();
    reopened.save_now().unwrap();
    assert!(!path.exists());
}
