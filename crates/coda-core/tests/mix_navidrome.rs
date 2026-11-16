//! A Navidrome mix resolved against a local `wiremock` server: genres,
//! artists and albums, the id fallback by name, failures, and the
//! coordinator's bookkeeping. No test contacts a real server.

use std::sync::Arc;
use std::time::{Duration, Instant};

use coda_core::mix::navidrome::{
    ALBUM_FETCHES, Fetch, Fetched, Finished, MAX_ARTIST_ALBUMS, MAX_ATTEMPTS, MAX_POOL_TRACKS,
    MixFetches, RETRY_AFTER, ResolveError,
};
use coda_core::mix::{ActiveMix, Ingredient, Mix, MixSource, Picker, StartError, no_songs_message};
use coda_core::navidrome::api::RANDOM_SONGS_SIZE;
use coda_core::navidrome::browse::MAX_CONCURRENT_FETCHES;
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password, server_id};
use coda_core::player::Player;
use coda_core::queue::Rng;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": {
        "status": "failed", "version": "1.16.1",
        "error": { "code": 70, "message": "data not found" }
    }}))
}

fn client_for(server: &MockServer, user: &str, timeout: Duration) -> Client {
    Client::with_timeouts(
        parse_base_url(&server.uri()).unwrap(),
        user.into(),
        Password::new("s3cret-pass".into()),
        Duration::from_secs(5),
        timeout,
    )
    .unwrap()
}

async fn server() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = client_for(&server, "listener", Duration::from_secs(10));
    (server, client)
}

fn limit() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(4))
}

/// Runs `fetch` through [`Fetch::spawn`] (its one entry point) and waits
/// for its answer.
async fn run(fetch: Fetch, client: &Client, limit: Arc<Semaphore>) -> Fetched {
    let (tx, rx) = tokio::sync::oneshot::channel();
    fetch.spawn(client.clone(), limit, move |done| {
        let _ = tx.send(done);
    });
    rx.await.unwrap()
}

/// The ids of the albums requested, in request order.
async fn albums_asked(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/rest/getAlbum")
        .filter_map(|r| {
            r.url
                .query_pairs()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.into_owned())
        })
        .collect()
}

/// `n` songs of album `album`, ids `<album>-<i>`.
fn songs(album: &str, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            json!({
                "id": format!("{album}-{i}"), "title": format!("Song {i}"),
                "album": album, "albumId": album, "artist": "Yes"
            })
        })
        .collect()
}

fn album_reply(id: &str, n: usize) -> ResponseTemplate {
    ok(json!({ "album": { "id": id, "name": id, "song": songs(id, n) } }))
}

fn artist_reply(id: &str, albums: &[&str]) -> ResponseTemplate {
    let albums: Vec<Value> = albums
        .iter()
        .map(|a| json!({ "id": a, "name": a, "artist": "Yes" }))
        .collect();
    ok(json!({ "artist": { "id": id, "name": "Yes", "album": albums } }))
}

fn random_reply(prefix: &str, n: usize) -> ResponseTemplate {
    ok(json!({ "randomSongs": { "song": songs(prefix, n) } }))
}

async fn requests(server: &MockServer, endpoint: &str) -> usize {
    let wanted = format!("/rest/{endpoint}");
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == wanted)
        .count()
}

fn mix(client: &Client, ingredients: Vec<Ingredient>) -> Mix {
    Mix::new(
        "Test mix",
        MixSource::Navidrome {
            server: server_id(client),
        },
        ingredients,
    )
    .unwrap()
}

fn artist(name: &str, id: Option<&str>) -> Ingredient {
    Ingredient::Artist {
        name: name.into(),
        id: id.map(Into::into),
    }
}

fn album(artist: &str, name: &str, id: Option<&str>) -> Ingredient {
    Ingredient::Album {
        artist: artist.into(),
        name: name.into(),
        id: id.map(Into::into),
    }
}

fn genre(name: &str) -> Ingredient {
    Ingredient::Genre { name: name.into() }
}

/// A mix with its coordinator, before it starts.
fn setup(
    client: &Client,
    ingredients: Vec<Ingredient>,
    player: &Player,
) -> (ActiveMix, MixFetches) {
    let mix = mix(client, ingredients);
    let fetches = MixFetches::new(&mix, client).unwrap();
    let n = mix.ingredients.len();
    (ActiveMix::new(mix, true, Picker::new(n), player), fetches)
}

/// Runs every due fetch to its end and applies the results, in order.
async fn run_due(
    client: &Client,
    active: &mut ActiveMix,
    fetches: &mut MixFetches,
    player: &Player,
    now: Instant,
) -> Vec<Finished> {
    let due = fetches.due(active, player, now);
    let mut finished = Vec::new();
    for fetch in due {
        let done = run(fetch, client, limit()).await;
        finished.push(fetches.finish(done, active, player, now));
    }
    finished
}

/// One fetch run alone, for its tracks.
async fn resolve_one(client: &Client, ingredient: Ingredient) -> (usize, Finished) {
    let player = Player::new();
    let (mut active, mut fetches) = setup(client, vec![ingredient], &player);
    let finished = run_due(client, &mut active, &mut fetches, &player, Instant::now()).await;
    assert_eq!(finished.len(), 1);
    let songs = match finished[0] {
        Finished::Pool { songs, .. } => songs,
        _ => 0,
    };
    (songs, finished.into_iter().next().unwrap())
}

#[tokio::test]
async fn a_genre_is_asked_for_by_its_exact_value_and_fetched_again_once_played_through() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Rock & Roll "))
        .respond_with(random_reply("first", RANDOM_SONGS_SIZE))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .and(query_param("genre", "Rock & Roll "))
        .respond_with(random_reply("second", RANDOM_SONGS_SIZE))
        .mount(&server)
        .await;
    let mut player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![genre("Rock & Roll ")], &player);
    let now = Instant::now();

    let finished = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(
        finished,
        vec![Finished::Pool {
            index: 0,
            songs: RANDOM_SONGS_SIZE
        }]
    );
    assert!(fetches.settled(&active));
    let mut rng = Rng::from_seed(3);
    active.start(&mut player, &mut rng).unwrap();
    assert!(
        fetches.due(&active, &player, now).is_empty(),
        "nothing due before the sample is played through"
    );

    // Play the rest of the sample through.
    let picked = active.picker_mut().next(RANDOM_SONGS_SIZE - 25, &mut rng);
    assert!(picked.iter().all(|t| t.title.starts_with("Song")));
    assert_eq!(active.picker().played_through(), vec![0]);

    let finished = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(
        finished,
        vec![Finished::Pool {
            index: 0,
            songs: RANDOM_SONGS_SIZE
        }]
    );
    assert!(active.picker().played_through().is_empty());
    let next = active.picker_mut().next(10, &mut rng);
    assert!(
        next.iter()
            .all(|t| t.navidrome_id().unwrap().starts_with("second-")),
        "the new sample plays"
    );
    assert_eq!(requests(&server, "getRandomSongs").await, 2);
}

#[tokio::test]
async fn a_genre_smaller_than_a_sample_is_never_fetched_again() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("g", 3))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![genre("Jazz")], &player);
    let now = Instant::now();
    run_due(&client, &mut active, &mut fetches, &player, now).await;
    active.picker_mut().next(10, &mut Rng::from_seed(1));
    assert_eq!(active.picker().played_through(), vec![0]);
    assert!(fetches.due(&active, &player, now + RETRY_AFTER).is_empty());
    assert_eq!(requests(&server, "getRandomSongs").await, 1);
}

#[tokio::test]
async fn an_artist_is_its_albums_songs_fetched_once() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar1"))
        .respond_with(artist_reply("ar1", &["al1", "al2", "al3"]))
        .mount(&server)
        .await;
    for id in ["al1", "al2", "al3"] {
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum"))
            .and(query_param("id", id))
            .respond_with(album_reply(id, 2))
            .mount(&server)
            .await;
    }
    let mut player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let now = Instant::now();
    let finished = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(finished, vec![Finished::Pool { index: 0, songs: 6 }]);
    let mut rng = Rng::from_seed(5);
    active.start(&mut player, &mut rng).unwrap();
    active.picker_mut().next(20, &mut rng);
    assert_eq!(active.picker().played_through(), vec![0]);
    assert!(
        fetches
            .due(&active, &player, now + RETRY_AFTER * 10)
            .is_empty(),
        "an artist is kept for as long as the mix plays"
    );
    assert_eq!(requests(&server, "getArtist").await, 1);
    assert_eq!(requests(&server, "getAlbum").await, 3);
    assert_eq!(requests(&server, "search3").await, 0);
}

#[tokio::test]
async fn an_artist_fetches_at_most_its_first_albums() {
    let (server, client) = server().await;
    let ids: Vec<String> = (0..MAX_ARTIST_ALBUMS + 5)
        .map(|i| format!("al{i}"))
        .collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &ids))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("any", 1))
        .mount(&server)
        .await;
    let (songs, _) = resolve_one(&client, artist("Yes", Some("ar1"))).await;
    assert_eq!(songs, MAX_ARTIST_ALBUMS, "one song from each album fetched");
    assert_eq!(requests(&server, "getAlbum").await, MAX_ARTIST_ALBUMS);
}

#[tokio::test]
async fn an_artists_albums_are_fetched_a_few_at_a_time() {
    let (server, client) = server().await;
    let ids = ["al1", "al2", "al3", "al4"];
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &ids))
        .mount(&server)
        .await;
    let delay = Duration::from_millis(200);
    for id in ids {
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum"))
            .and(query_param("id", id))
            .respond_with(album_reply(id, 1).set_delay(delay))
            .mount(&server)
            .await;
    }
    let started = Instant::now();
    let (songs, _) = resolve_one(&client, artist("Yes", Some("ar1"))).await;
    assert_eq!(songs, 4);
    // Four albums, `ALBUM_FETCHES` (2) at a time: at least two waves.
    assert_eq!(ALBUM_FETCHES, 2);
    assert!(started.elapsed() >= delay * 2 - Duration::from_millis(20));
}

#[tokio::test]
async fn every_request_waits_for_the_callers_limit() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .respond_with(artist_reply("ar1", &["al1"]))
        .mount(&server)
        .await;
    let player = Player::new();
    let (active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let fetch = fetches.due(&active, &player, Instant::now()).remove(0);
    let closed = Arc::new(Semaphore::new(0));
    let answer = tokio::time::timeout(
        Duration::from_millis(300),
        run(fetch, &client, Arc::clone(&closed)),
    )
    .await;
    assert!(answer.is_err(), "no permit, no request");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn an_album_is_its_songs() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al7"))
        .respond_with(album_reply("al7", 8))
        .mount(&server)
        .await;
    let (songs, finished) = resolve_one(&client, album("Yes", "Fragile", Some("al7"))).await;
    assert_eq!(songs, 8);
    assert_eq!(finished, Finished::Pool { index: 0, songs: 8 });
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_artist_whose_id_is_gone_is_found_by_name() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "old"))
        .respond_with(not_found())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "Yes"))
        .respond_with(ok(json!({ "searchResult3": { "artist": [
            { "id": "ar-yesterday", "name": "Yesterday's Kids" },
            { "id": "ar-new", "name": "  YES " }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar-new"))
        .respond_with(artist_reply("ar-new", &["al1"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(album_reply("al1", 4))
        .mount(&server)
        .await;
    let (songs, _) = resolve_one(&client, artist("Yes", Some("old"))).await;
    assert_eq!(songs, 4);
    assert_eq!(requests(&server, "getArtist").await, 2);
    assert_eq!(requests(&server, "search3").await, 1);
}

#[tokio::test]
async fn an_artist_without_an_id_is_found_by_name_directly() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .respond_with(ok(json!({ "searchResult3": { "artist": [
            { "id": "ar-new", "name": "yes" }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar-new"))
        .respond_with(artist_reply("ar-new", &["al1"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 2))
        .mount(&server)
        .await;
    let (songs, _) = resolve_one(&client, artist("Yes", None)).await;
    assert_eq!(songs, 2);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn an_album_whose_id_is_gone_is_found_by_name_and_artist() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "old"))
        .respond_with(not_found())
        .mount(&server)
        .await;
    // By the album name: only someone else's "Fragile".
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "Fragile"))
        .respond_with(ok(json!({ "searchResult3": { "album": [
            { "id": "al-other", "name": "Fragile", "artist": "Somebody" }
        ]}})))
        .mount(&server)
        .await;
    // By artist and album name: the right one.
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "Yes Fragile"))
        .respond_with(ok(json!({ "searchResult3": { "album": [
            { "id": "al-other", "name": "Fragile", "artist": "Somebody" },
            { "id": "al-new", "name": "fragile", "artist": "YES" }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al-new"))
        .respond_with(album_reply("al-new", 5))
        .mount(&server)
        .await;
    let (songs, _) = resolve_one(&client, album("Yes", "Fragile", Some("old"))).await;
    assert_eq!(songs, 5);
    assert_eq!(requests(&server, "search3").await, 2);
    assert_eq!(requests(&server, "getAlbum").await, 2);
}

#[tokio::test]
async fn an_ingredient_found_nowhere_has_no_songs_and_the_rest_plays() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(album_reply("al1", 3))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(not_found())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtists"))
        .respond_with(ok(json!({ "artists": { "index": [] } })))
        .mount(&server)
        .await;
    let mut player = Player::new();
    let (mut active, mut fetches) = setup(
        &client,
        vec![
            album("Yes", "Fragile", Some("al1")),
            artist("Nobody", Some("gone")),
            album("Nobody", "Nothing", None),
        ],
        &player,
    );
    let finished = run_due(&client, &mut active, &mut fetches, &player, Instant::now()).await;
    assert_eq!(
        finished,
        vec![
            Finished::Pool { index: 0, songs: 3 },
            Finished::NoSongs { index: 1 },
            Finished::NoSongs { index: 2 },
        ]
    );
    assert!(fetches.settled(&active));
    assert_eq!(
        no_songs_message(active.empty_ingredients()).as_deref(),
        Some("No songs for artist Nobody, album Nothing by Nobody")
    );
    active.start(&mut player, &mut Rng::from_seed(1)).unwrap();
    // One search for the artist, two for the album without an id.
    assert_eq!(requests(&server, "search3").await, 3);
    assert_eq!(requests(&server, "getArtists").await, 1);
}

#[tokio::test]
async fn a_mix_never_resolves_against_another_server() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .respond_with(random_reply("g", 3))
        .mount(&server)
        .await;
    let other = client_for(&server, "someone-else", Duration::from_secs(10));
    let made_elsewhere = mix(&other, vec![genre("Jazz")]);
    assert!(matches!(
        MixFetches::new(&made_elsewhere, &client),
        Err(ResolveError::OtherServer)
    ));
    let local = Mix::new("Local", MixSource::Local, vec![genre("Jazz")]).unwrap();
    assert!(matches!(
        MixFetches::new(&local, &client),
        Err(ResolveError::NotNavidrome)
    ));

    // A fetch made for one server, run with another client.
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![genre("Jazz")], &player);
    let now = Instant::now();
    let fetch = fetches.due(&active, &player, now).remove(0);
    let done = run(fetch, &other, limit()).await;
    assert_eq!(
        fetches.finish(done, &mut active, &player, now),
        Finished::OtherServer { index: 0 }
    );
    assert!(
        fetches.other_server(),
        "terminal: the frontend ends the mix"
    );
    assert!(!fetches.settled(&active));
    assert!(fetches.due(&active, &player, now + RETRY_AFTER).is_empty());
    assert_eq!(fetches.next_retry(&active), None);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_failed_fetch_is_retried_after_a_while_a_few_times() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) =
        setup(&client, vec![album("Yes", "Fragile", Some("al1"))], &player);
    let mut now = Instant::now();
    let finished = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(finished, vec![Finished::Failed { index: 0 }]);
    assert!(
        !fetches.settled(&active),
        "no songs yet and attempts left: not settled"
    );
    assert_eq!(fetches.next_retry(&active), Some(now + RETRY_AFTER));
    assert_eq!(fetches.failed(&active), vec![0]);
    assert!(active.picker().empty().is_empty(), "a failure is not empty");
    assert!(
        fetches
            .due(&active, &player, now + RETRY_AFTER / 2)
            .is_empty()
    );
    for _ in 1..MAX_ATTEMPTS {
        now += RETRY_AFTER;
        let finished = run_due(&client, &mut active, &mut fetches, &player, now).await;
        assert_eq!(finished, vec![Finished::Failed { index: 0 }]);
    }
    assert!(
        fetches
            .due(&active, &player, now + RETRY_AFTER * 10)
            .is_empty(),
        "given up after {MAX_ATTEMPTS} attempts"
    );
    assert!(fetches.settled(&active), "every attempt used: settled");
    assert_eq!(fetches.next_retry(&active), None);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        MAX_ATTEMPTS as usize
    );
}

#[tokio::test]
async fn an_artist_skips_a_failing_album_and_a_timeout_fails_it_whole() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar1"))
        .respond_with(artist_reply("ar1", &["al1", "al2"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(album_reply("al1", 2))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al2"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let (_, finished) = resolve_one(&client, artist("Yes", Some("ar1"))).await;
    assert_eq!(
        finished,
        Finished::Pool { index: 0, songs: 2 },
        "the album that failed is skipped, the other plays"
    );

    // Every album failing: the artist fails (and is retried).
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar2"))
        .respond_with(artist_reply("ar2", &["al2"]))
        .mount(&server)
        .await;
    let (_, finished) = resolve_one(&client, artist("Yes", Some("ar2"))).await;
    assert_eq!(finished, Finished::Failed { index: 0 });

    // A network failure (a timeout) on one album fails the artist whole.
    let slow = client_for(&server, "listener", Duration::from_millis(200));
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar3"))
        .respond_with(artist_reply("ar3", &["al1", "al-slow"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al-slow"))
        .respond_with(album_reply("al-slow", 2).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let (_, finished) = resolve_one(&slow, artist("Yes", Some("ar3"))).await;
    assert_eq!(finished, Finished::Failed { index: 0 });

    // A refused account or a redirect on one album fails it whole too:
    // every album would answer the same, so no partial pool is kept.
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al-auth"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": {
                "status": "failed", "version": "1.16.1",
                "error": { "code": 40, "message": "wrong username or password" }
            }})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al-moved"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "http://elsewhere/"))
        .mount(&server)
        .await;
    for (id, bad) in [("ar4", "al-auth"), ("ar5", "al-moved")] {
        Mock::given(method("GET"))
            .and(path("/rest/getArtist"))
            .and(query_param("id", id))
            .respond_with(artist_reply(id, &["al1", bad]))
            .mount(&server)
            .await;
        let (_, finished) = resolve_one(&client, artist("Yes", Some(id))).await;
        assert_eq!(finished, Finished::Failed { index: 0 }, "{bad}");
    }

    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("g", 3).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let (_, finished) = resolve_one(&slow, genre("Slow")).await;
    assert_eq!(finished, Finished::Failed { index: 0 });
}

#[tokio::test]
async fn a_refetch_failure_or_empty_sample_keeps_the_pool_that_plays() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("first", RANDOM_SONGS_SIZE))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("none", 0))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![genre("Jazz")], &player);
    let mut now = Instant::now();
    run_due(&client, &mut active, &mut fetches, &player, now).await;
    let mut rng = Rng::from_seed(2);
    active.picker_mut().next(RANDOM_SONGS_SIZE, &mut rng);
    let failed = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(failed, vec![Finished::Failed { index: 0 }]);
    assert!(fetches.failed(&active).is_empty(), "its songs arrived once");
    assert!(fetches.due(&active, &player, now).is_empty(), "not yet");
    now += RETRY_AFTER;
    let empty = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(empty, vec![Finished::Unchanged { index: 0 }]);
    assert_eq!(active.picker_mut().next(5, &mut rng).len(), 5);
    assert!(fetches.due(&active, &player, now).is_empty(), "not yet");
    assert_eq!(fetches.next_retry(&active), Some(now + RETRY_AFTER));
    assert_eq!(requests(&server, "getRandomSongs").await, 3);

    // An empty sample is not the whole genre: asked again later, and the
    // recovered sample plays.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("back", RANDOM_SONGS_SIZE))
        .mount(&server)
        .await;
    now += RETRY_AFTER;
    let back = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(
        back,
        vec![Finished::Pool {
            index: 0,
            songs: RANDOM_SONGS_SIZE
        }]
    );
    assert!(
        active
            .picker_mut()
            .next(5, &mut rng)
            .iter()
            .all(|t| t.navidrome_id().unwrap().starts_with("back-"))
    );
}

#[tokio::test]
async fn a_result_after_the_queue_was_replaced_is_dropped() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(album_reply("al1", 30))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("g", RANDOM_SONGS_SIZE))
        .mount(&server)
        .await;
    let mut player = Player::new();
    let (mut active, mut fetches) = setup(
        &client,
        vec![album("Yes", "Fragile", Some("al1")), genre("Jazz")],
        &player,
    );
    let now = Instant::now();
    let mut due = fetches.due(&active, &player, now);
    assert_eq!(due.len(), 2);
    assert!(
        fetches.due(&active, &player, now).is_empty(),
        "one fetch per ingredient in flight"
    );
    assert_eq!(fetches.in_flight(), 2);
    let late = run(due.pop().unwrap(), &client, limit()).await;
    let first = run(due.pop().unwrap(), &client, limit()).await;
    assert_eq!(
        fetches.finish(first, &mut active, &player, now),
        Finished::Pool {
            index: 0,
            songs: 30
        }
    );
    assert!(!fetches.settled(&active), "the genre is still out");
    active.start(&mut player, &mut Rng::from_seed(1)).unwrap();

    // Playing an album replaces the queue: the mix is over.
    let other = active.picker_mut().next(3, &mut Rng::from_seed(9));
    let _ = player.play_context(other, 0);
    assert!(active.has_ended(&player));
    assert_eq!(
        fetches.finish(late, &mut active, &player, now),
        Finished::Stale
    );
    assert_eq!(active.picker().pending(), vec![1], "the genre was not set");
    assert!(fetches.due(&active, &player, now).is_empty());
}

#[tokio::test]
async fn a_result_for_another_coordinator_is_dropped() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .respond_with(album_reply("al1", 3))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut old) = setup(&client, vec![album("Yes", "Fragile", Some("al1"))], &player);
    let now = Instant::now();
    let fetch = old.due(&active, &player, now).remove(0);
    let done: Fetched = run(fetch, &client, limit()).await;
    // The same mix played again: a new coordinator.
    let mut new = MixFetches::new(active.mix(), &client).unwrap();
    let again = new.due(&active, &player, now);
    assert_eq!(again.len(), 1);
    assert_eq!(new.finish(done, &mut active, &player, now), Finished::Stale);
    assert!(!active.picker().is_ready());
    // An aborted task's answer counts as a failure of its own fetch.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let fetch = again.into_iter().next().unwrap();
    fetch
        .spawn(client.clone(), limit(), move |done| {
            let _ = tx.send(done);
        })
        .abort();
    let aborted = rx.await.unwrap();
    assert_eq!(
        new.finish(aborted, &mut active, &player, now),
        Finished::Failed { index: 0 }
    );
}

#[tokio::test]
async fn a_mix_has_at_most_a_few_requests_in_flight_across_its_ingredients() {
    let (server, client) = server().await;
    let delay = Duration::from_millis(100);
    for id in ["ar1", "ar2", "ar3"] {
        let albums = [format!("{id}-a"), format!("{id}-b")];
        let albums: Vec<&str> = albums.iter().map(String::as_str).collect();
        Mock::given(method("GET"))
            .and(path("/rest/getArtist"))
            .and(query_param("id", id))
            .respond_with(artist_reply(id, &albums).set_delay(delay))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("any", 1).set_delay(delay))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) = setup(
        &client,
        vec![
            artist("One", Some("ar1")),
            artist("Two", Some("ar2")),
            artist("Three", Some("ar3")),
        ],
        &player,
    );
    // The frontend's shared limit: a mix takes at most `ALBUM_FETCHES`.
    let shared = Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES));
    let now = Instant::now();
    let due = fetches.due(&active, &player, now);
    assert_eq!(due.len(), 3, "every artist at once");
    let (tx, mut rx) = mpsc::unbounded_channel();
    for fetch in due {
        let tx = tx.clone();
        fetch.spawn(client.clone(), Arc::clone(&shared), move |done| {
            let _ = tx.send(done);
        });
    }
    let started = Instant::now();
    let mut results = Vec::new();
    let mut fewest_free = MAX_CONCURRENT_FETCHES;
    while results.len() < 3 {
        tokio::select! {
            Some(done) = rx.recv() => results.push(done),
            () = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
        fewest_free = fewest_free.min(shared.available_permits());
    }
    assert_eq!(
        fewest_free,
        MAX_CONCURRENT_FETCHES - ALBUM_FETCHES,
        "never more than {ALBUM_FETCHES} of the mix's requests in flight"
    );
    // 3 artists and 6 albums, `ALBUM_FETCHES` (2) at a time: 5 waves.
    assert!(started.elapsed() >= delay * 4);
    for done in results {
        assert!(matches!(
            fetches.finish(done, &mut active, &player, now),
            Finished::Pool { songs: 2, .. }
        ));
    }
    assert!(fetches.settled(&active));
    assert_eq!(requests(&server, "getArtist").await, 3);
    assert_eq!(requests(&server, "getAlbum").await, 6);
}

#[tokio::test]
async fn a_full_sample_with_an_unusable_song_is_not_the_whole_genre() {
    let (server, client) = server().await;
    let mut served = songs("first", RANDOM_SONGS_SIZE - 1);
    served.push(json!({ "title": "No id" }));
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ok(json!({ "randomSongs": { "song": served } })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(random_reply("second", RANDOM_SONGS_SIZE))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![genre("Rock")], &player);
    let now = Instant::now();
    let first = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(
        first,
        vec![Finished::Pool {
            index: 0,
            songs: RANDOM_SONGS_SIZE - 1
        }]
    );
    active
        .picker_mut()
        .next(RANDOM_SONGS_SIZE - 1, &mut Rng::from_seed(4));
    assert_eq!(active.picker().played_through(), vec![0]);
    let again = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(
        again,
        vec![Finished::Pool {
            index: 0,
            songs: RANDOM_SONGS_SIZE
        }],
        "500 served: sampled again"
    );
    assert_eq!(requests(&server, "getRandomSongs").await, 2);
}

#[tokio::test]
async fn a_mix_whose_only_fetch_failed_waits_for_the_retry_and_then_starts() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &["al1"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 4))
        .mount(&server)
        .await;
    let mut player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let now = Instant::now();
    let first = run_due(&client, &mut active, &mut fetches, &player, now).await;
    assert_eq!(first, vec![Finished::Failed { index: 0 }]);
    assert!(!fetches.settled(&active), "a retry is still to come");
    let retry = fetches.next_retry(&active).unwrap();
    assert_eq!(retry, now + RETRY_AFTER);
    assert!(fetches.due(&active, &player, now).is_empty());
    let second = run_due(&client, &mut active, &mut fetches, &player, retry).await;
    assert_eq!(second, vec![Finished::Pool { index: 0, songs: 4 }]);
    assert!(fetches.settled(&active));
    assert_eq!(fetches.next_retry(&active), None);
    active.start(&mut player, &mut Rng::from_seed(1)).unwrap();
    assert!(!player.queue().is_empty(), "the mix plays");
}

#[tokio::test]
async fn a_spawned_fetch_answers_once_even_when_aborted() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 3).set_delay(Duration::from_secs(5)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 3))
        .mount(&server)
        .await;
    let player = Player::new();
    let (mut active, mut fetches) =
        setup(&client, vec![album("Yes", "Fragile", Some("al1"))], &player);
    let now = Instant::now();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let send = |tx: &mpsc::UnboundedSender<Fetched>| {
        let tx = tx.clone();
        move |done| {
            let _ = tx.send(done);
        }
    };
    let fetch = fetches.due(&active, &player, now).remove(0);
    let task = fetch.spawn(client.clone(), limit(), send(&tx));
    // Let the request start, then abort the task.
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    let aborted = rx.recv().await.unwrap();
    assert_eq!(
        fetches.finish(aborted, &mut active, &player, now),
        Finished::Failed { index: 0 }
    );
    assert_eq!(fetches.in_flight(), 0, "nothing left in flight");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err(), "answered once");

    // Dropped before it ever ran.
    let later = now + RETRY_AFTER;
    let fetch = fetches.due(&active, &player, later).remove(0);
    fetch.spawn(client.clone(), limit(), send(&tx)).abort();
    let aborted = rx.recv().await.unwrap();
    assert_eq!(
        fetches.finish(aborted, &mut active, &player, later),
        Finished::Failed { index: 0 }
    );

    // A task that runs to its end answers with its songs.
    let last = later + RETRY_AFTER;
    let fetch = fetches.due(&active, &player, last).remove(0);
    fetch.spawn(client.clone(), limit(), send(&tx));
    let done = rx.recv().await.unwrap();
    assert_eq!(
        fetches.finish(done, &mut active, &player, last),
        Finished::Pool { index: 0, songs: 3 }
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err(), "answered once");
}

#[tokio::test]
async fn a_queue_replaced_before_the_mix_started_ends_it() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 5))
        .mount(&server)
        .await;
    let mut player = Player::new();
    let (mut active, mut fetches) =
        setup(&client, vec![album("Yes", "Fragile", Some("al1"))], &player);
    let now = Instant::now();
    let fetch = fetches.due(&active, &player, now).remove(0);
    let done = run(fetch, &client, limit()).await;
    assert!(!active.has_ended(&player));

    // While the mix resolves, the user plays an album.
    let album_songs = client.album("al1").await.unwrap();
    let _ = player.play_context(album_songs, 0);
    assert!(active.has_ended(&player));
    assert_eq!(
        fetches.finish(done, &mut active, &player, now),
        Finished::Stale
    );
    assert!(fetches.due(&active, &player, now + RETRY_AFTER).is_empty());
    let queue = player.queue().len();
    assert_eq!(
        active.start(&mut player, &mut Rng::from_seed(1)),
        Err(StartError::Ended)
    );
    assert_eq!(player.queue().len(), queue, "the user's queue stays");
}

#[tokio::test]
async fn an_artist_missing_from_the_search_is_found_in_the_index() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .respond_with(ok(json!({ "searchResult3": { "artist": [
            { "id": "ar-men", "name": "Yes Men" }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtists"))
        .respond_with(ok(json!({ "artists": { "index": [
            { "name": "Y", "artist": [
                { "id": "ar-men", "name": "Yes Men" },
                { "id": "ar-yes", "name": "Yes" }
            ]}
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar-yes"))
        .respond_with(artist_reply("ar-yes", &["al1"]))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("al1", 3))
        .mount(&server)
        .await;
    let (songs, _) = resolve_one(&client, artist("yes", None)).await;
    assert_eq!(songs, 3);
    assert_eq!(requests(&server, "search3").await, 1);
    assert_eq!(requests(&server, "getArtists").await, 1);
    assert_eq!(requests(&server, "getArtist").await, 1);
}

#[tokio::test]
async fn an_artist_stops_fetching_albums_once_its_pool_is_full() {
    let (server, client) = server().await;
    let ids = ["al1", "al2", "al3", "al4", "al5"];
    let big = MAX_POOL_TRACKS * 6 / 10;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &ids))
        .mount(&server)
        .await;
    for id in ids {
        let mut reply = album_reply(id, big);
        if id == "al3" {
            // Still in flight when the pool fills: aborted, not awaited.
            reply = reply.set_delay(Duration::from_secs(5));
        }
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum"))
            .and(query_param("id", id))
            .respond_with(reply)
            .mount(&server)
            .await;
    }
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let started = Instant::now();
    let finished = run_due(&client, &mut active, &mut fetches, &player, started).await;
    assert_eq!(
        finished,
        vec![Finished::Pool {
            index: 0,
            songs: MAX_POOL_TRACKS
        }]
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the album in flight was not awaited"
    );
    // In album order: all of al1, then the start of al2.
    let picked = active.picker_mut().next(2_000, &mut Rng::from_seed(8));
    assert!(picked.iter().all(|t| {
        let id = t.navidrome_id().unwrap();
        id.starts_with("al1-")
            || id
                .strip_prefix("al2-")
                .is_some_and(|n| n.parse::<usize>().unwrap() < MAX_POOL_TRACKS - big)
    }));
    let asked = albums_asked(&server).await;
    assert!(asked.len() <= 3, "{asked:?}");
    assert!(!asked.iter().any(|id| id == "al4" || id == "al5"));
}

#[tokio::test]
async fn a_slow_first_album_holds_back_the_albums_after_it() {
    let (server, client) = server().await;
    let ids = ["al1", "al2", "al3", "al4", "al5"];
    let big = MAX_POOL_TRACKS * 6 / 10;
    let slow = Duration::from_millis(800);
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &ids))
        .mount(&server)
        .await;
    for id in ids {
        let mut reply = album_reply(id, big);
        if id == "al1" {
            reply = reply.set_delay(slow);
        }
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum"))
            .and(query_param("id", id))
            .respond_with(reply)
            .mount(&server)
            .await;
    }
    let player = Player::new();
    let (mut active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let now = Instant::now();
    let fetch = fetches.due(&active, &player, now).remove(0);
    let (tx, rx) = tokio::sync::oneshot::channel();
    fetch.spawn(client.clone(), limit(), move |done| {
        let _ = tx.send(done);
    });
    // While al1 is pending, the albums after it stay within the window.
    tokio::time::sleep(slow / 2).await;
    let asked = albums_asked(&server).await;
    assert_eq!(asked.len(), ALBUM_FETCHES, "{asked:?}");
    let done = rx.await.unwrap();
    assert_eq!(
        fetches.finish(done, &mut active, &player, now),
        Finished::Pool {
            index: 0,
            songs: MAX_POOL_TRACKS
        }
    );
    // In album order: all of al1, then the start of al2.
    let picked = active.picker_mut().next(2_000, &mut Rng::from_seed(8));
    assert!(
        picked
            .iter()
            .any(|t| t.navidrome_id().unwrap().starts_with("al1-"))
    );
    assert!(picked.iter().all(|t| {
        let id = t.navidrome_id().unwrap();
        id.starts_with("al1-")
            || id
                .strip_prefix("al2-")
                .is_some_and(|n| n.parse::<usize>().unwrap() < MAX_POOL_TRACKS - big)
    }));
    let asked = albums_asked(&server).await;
    assert!(asked.len() <= ALBUM_FETCHES + 1, "{asked:?}");
    assert!(
        !asked.iter().any(|id| id == "al4" || id == "al5"),
        "{asked:?}"
    );
}

#[tokio::test]
async fn dropping_the_coordinator_stops_its_fetches() {
    let (server, client) = server().await;
    let ids: Vec<String> = (1..=10).map(|i| format!("al{i}")).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let delay = Duration::from_millis(300);
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(artist_reply("ar1", &ids))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(album_reply("any", 1).set_delay(delay))
        .mount(&server)
        .await;
    let player = Player::new();
    let (active, mut fetches) = setup(&client, vec![artist("Yes", Some("ar1"))], &player);
    let shared = Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES));
    let fetch = fetches.due(&active, &player, Instant::now()).remove(0);
    let (tx, rx) = tokio::sync::oneshot::channel();
    fetch.spawn(client.clone(), Arc::clone(&shared), move |done| {
        let _ = tx.send(done);
    });
    // The first albums are in flight; the mix is replaced.
    tokio::time::sleep(delay / 2).await;
    assert_eq!(requests(&server, "getAlbum").await, ALBUM_FETCHES);
    let started = Instant::now();
    drop(fetches);
    // The fetch ends once those in flight answer, without the other
    // albums (10 albums, two at a time, would take five waves).
    let done = tokio::time::timeout(delay * 3, rx).await.unwrap().unwrap();
    assert!(started.elapsed() < delay * 2);
    assert_eq!(requests(&server, "getAlbum").await, ALBUM_FETCHES);
    assert_eq!(shared.available_permits(), MAX_CONCURRENT_FETCHES);
    // Its answer is for no coordinator any more.
    let mut active = active;
    let mut other = MixFetches::new(active.mix(), &client).unwrap();
    assert_eq!(
        other.finish(done, &mut active, &player, Instant::now()),
        Finished::Stale
    );
}
