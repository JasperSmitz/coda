//! Navidrome cover art against a local `wiremock` server: `coverArt` ids in
//! every endpoint that has them, `getCoverArt` responses classified by
//! their bytes, and the disk cache in between. No test contacts a real
//! server.

use std::error::Error as _;
use std::sync::Arc;

use coda_tui::cover::cache::{DiskCache, large_dir};
use coda_tui::cover::{
    COVER_SIZE, CoverKey, CoverRequest, CoverRequestId, CoverSize, LARGE_COVER_SIZE, Outcome,
    spawn_load,
};
use coda_tui::navidrome::browse::Generation;
use coda_tui::navidrome::client::{MAX_COVER_BYTES, parse_base_url};
use coda_tui::navidrome::{Client, Error, Password};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, RgbImage};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PASSWORD: &str = "s3cret-pass";
const USER: &str = "listener";

async fn server() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        USER.into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    (server, client)
}

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn pixels(size: u32) -> RgbImage {
    RgbImage::from_fn(size, size, |x, y| image::Rgb([x as u8, y as u8, 99]))
}

fn jpeg(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&pixels(size), size, size, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn webp(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&pixels(size), size, size, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

async fn cover(server: &MockServer, id: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt"))
        .and(query_param("id", id))
        .respond_with(response)
        .mount(server)
        .await;
}

fn assert_secret_free(err: &Error) {
    let mut texts = vec![err.to_string(), format!("{err:?}"), err.detail()];
    let mut source = err.source();
    while let Some(s) = source {
        texts.push(s.to_string());
        texts.push(format!("{s:?}"));
        source = s.source();
    }
    for text in texts {
        for needle in [PASSWORD, USER, "t=", "s=", "u=", "rest/", "?", "al-"] {
            assert!(!text.contains(needle), "{needle:?} leaked in {text:?}");
        }
    }
}

#[tokio::test]
async fn cover_ids_come_from_songs_and_albums_in_every_endpoint() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(ok(json!({ "album": {
            "id": "al1", "name": "A", "coverArt": "al-1_aa",
            "song": [
                { "id": "s1", "coverArt": "mf-1_bb" },
                { "id": "s2" },
                { "id": "s3", "coverArt": "" },
                { "id": "s4", "coverArt": ["not", "an", "id"] },
                { "id": "s5", "coverArt": 77 },
            ]
        }})))
        .mount(&server)
        .await;
    let tracks = client.album("al1").await.unwrap();
    let ids: Vec<Option<&str>> = tracks.iter().map(|t| t.cover_art.as_deref()).collect();
    assert_eq!(
        ids,
        [
            Some("mf-1_bb"),
            Some("al-1_aa"),
            Some("al-1_aa"),
            Some("al-1_aa"),
            Some("77"),
        ],
        "a song without its own takes the album's"
    );

    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .respond_with(ok(json!({ "searchResult3": {
            "album": [{ "id": "al2", "name": "B", "coverArt": "al-2" }],
            "song": [{ "id": "s9", "coverArt": "mf-9" }, { "id": "s10" }]
        }})))
        .mount(&server)
        .await;
    let found = client.search("x").await.unwrap();
    assert_eq!(found.tracks[0].cover_art.as_deref(), Some("mf-9"));
    assert_eq!(found.tracks[1].cover_art, None, "no album to fall back on");

    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ok(json!({ "randomSongs": { "song": [
            { "id": "r1", "coverArt": "mf-r1" }
        ]}})))
        .mount(&server)
        .await;
    let random = client.random_songs().await.unwrap();
    assert_eq!(random[0].cover_art.as_deref(), Some("mf-r1"));

    Mock::given(method("GET"))
        .and(path("/rest/getPlaylist"))
        .respond_with(ok(json!({ "playlist": {
            "id": "p1", "name": "P", "coverArt": "pl-1",
            "entry": [{ "id": "e1", "coverArt": "mf-e1" }, { "id": "e2" }]
        }})))
        .mount(&server)
        .await;
    let playlist = client.playlist("p1").await.unwrap();
    assert_eq!(playlist.tracks[0].cover_art.as_deref(), Some("mf-e1"));
    assert_eq!(playlist.tracks[1].cover_art, None);
}

#[tokio::test]
async fn an_oversized_cover_id_is_dropped_not_the_song() {
    let (server, client) = server().await;
    let long = "x".repeat(300);
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs"))
        .respond_with(ok(json!({ "randomSongs": { "song": [
            { "id": "r1", "title": "Kept", "coverArt": long }
        ]}})))
        .mount(&server)
        .await;
    let random = client.random_songs().await.unwrap();
    assert_eq!(random[0].title, "Kept");
    assert_eq!(random[0].cover_art, None);
}

#[tokio::test]
async fn get_cover_art_asks_for_a_small_signed_image() {
    let (server, client) = server().await;
    cover(
        &server,
        "al-1_aa",
        ResponseTemplate::new(200)
            .insert_header("content-type", "image/jpeg")
            .insert_header("cache-control", "public, max-age=315360000, immutable")
            .set_body_bytes(jpeg(32)),
    )
    .await;
    let bytes = client.cover_art("al-1_aa", COVER_SIZE).await.unwrap();
    assert_eq!(bytes.bytes, jpeg(32));
    assert!(!bytes.no_store);
    let requests = server.received_requests().await.unwrap();
    let query: Vec<(String, String)> = requests[0]
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let get = |key: &str| {
        query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("id"), Some("al-1_aa"));
    assert_eq!(get("size"), Some("256"));
    assert_eq!(get("u"), Some(USER));
    assert!(get("t").is_some() && get("s").is_some());
    assert_eq!(get("p"), None, "never the password");
    // The Debug output shows the length, not the bytes.
    assert!(format!("{bytes:?}").starts_with("CoverBytes { len: "));
}

#[tokio::test]
async fn responses_are_classified_by_their_bytes() {
    let (server, client) = server().await;
    // WebP, labelled as anything.
    cover(
        &server,
        "webp",
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/octet-stream")
            .set_body_bytes(webp(16)),
    )
    .await;
    // Navidrome's answer for an unknown id: a Subsonic error, HTTP 200.
    cover(
        &server,
        "missing",
        ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": 70, "message": "Artwork not found" }
        }})),
    )
    .await;
    // Navidrome's placeholder for an item without art.
    cover(
        &server,
        "placeholder",
        ResponseTemplate::new(200)
            .insert_header("cache-control", "no-store")
            .set_body_bytes(jpeg(16)),
    )
    .await;
    // "image/jpeg" that is not one.
    cover(
        &server,
        "liar",
        ResponseTemplate::new(200)
            .insert_header("content-type", "image/jpeg")
            .set_body_bytes(b"<html>login</html>".to_vec()),
    )
    .await;
    cover(&server, "gone", ResponseTemplate::new(404)).await;
    cover(
        &server,
        "moved",
        ResponseTemplate::new(302).insert_header("location", "http://example.invalid/x"),
    )
    .await;
    let mut huge = jpeg(8);
    huge.resize(MAX_COVER_BYTES + 1, 0);
    cover(
        &server,
        "huge",
        ResponseTemplate::new(200).set_body_bytes(huge),
    )
    .await;

    let webp_bytes = client.cover_art("webp", 256).await.unwrap();
    assert_eq!(
        coda_tui::cover::sniff(&webp_bytes.bytes),
        Some(coda_tui::cover::Format::WebP)
    );
    let placeholder = client.cover_art("placeholder", 256).await.unwrap();
    assert!(placeholder.no_store);
    let errors = [
        client.cover_art("missing", 256).await.unwrap_err(),
        client.cover_art("liar", 256).await.unwrap_err(),
        client.cover_art("gone", 256).await.unwrap_err(),
        client.cover_art("moved", 256).await.unwrap_err(),
        client.cover_art("huge", 256).await.unwrap_err(),
    ];
    assert!(
        matches!(errors[0], Error::Server { code: 70, .. }),
        "{:?}",
        errors[0]
    );
    assert!(matches!(errors[1], Error::Decode(_)), "{:?}", errors[1]);
    assert!(matches!(errors[2], Error::Http(404)));
    assert!(matches!(errors[3], Error::Redirect(302)));
    assert!(matches!(errors[4], Error::TooLarge));
    for error in &errors {
        assert_secret_free(error);
    }
    // The redirect was not followed.
    let followed = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() != "/rest/getCoverArt")
        .count();
    assert_eq!(followed, 0);
}

async fn load(client: &Client, cache: &Arc<DiskCache>, id: &str) -> Outcome {
    load_at(client, cache, id, CoverSize::Small).await
}

async fn load_at(client: &Client, cache: &Arc<DiskCache>, id: &str, size: CoverSize) -> Outcome {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = CoverRequest {
        id: CoverRequestId(1),
        generation: Generation(0),
        key: CoverKey::Navidrome(id.into()),
        size,
    };
    spawn_load(request, Some(client.clone()), Some(cache.clone()), &tx)
        .await
        .unwrap();
    rx.recv().await.unwrap().outcome
}

fn cached(cache: &DiskCache) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(cache.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn covers_are_cached_once_and_placeholders_and_errors_never() {
    let (server, client) = server().await;
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(DiskCache::new(dir.path().join("covers")));
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt"))
        .and(query_param("id", "al-1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(600)))
        .expect(1)
        .mount(&server)
        .await;
    cover(
        &server,
        "placeholder",
        ResponseTemplate::new(200)
            .insert_header("cache-control", "private, no-store")
            .set_body_bytes(jpeg(16)),
    )
    .await;
    cover(
        &server,
        "missing",
        ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": 70, "message": "Artwork not found" }
        }})),
    )
    .await;

    // Fetched, scaled to 256 px and cached under a hashed name.
    let Outcome::Art(image) = load(&client, &cache, "al-1").await else {
        panic!("art expected");
    };
    assert_eq!((image.width(), image.height()), (COVER_SIZE, COVER_SIZE));
    let names = cached(&cache);
    assert_eq!(
        names,
        [DiskCache::name(client.base_url().as_str(), USER, "al-1")]
    );
    for secret in [USER, PASSWORD, "al-1", "127.0.0.1"] {
        assert!(!names[0].contains(secret), "{names:?}");
    }
    // The second time comes from the disk (the mock expects one request).
    assert!(matches!(
        load(&client, &cache, "al-1").await,
        Outcome::Art(_)
    ));
    // No art for the placeholder and the error, and nothing cached.
    assert!(matches!(
        load(&client, &cache, "placeholder").await,
        Outcome::NoArt
    ));
    assert!(matches!(
        load(&client, &cache, "missing").await,
        Outcome::NoArt
    ));
    assert_eq!(cached(&cache).len(), 1);
    // A broken cache entry is fetched and written again.
    let entry = cache.dir().join(&names[0]);
    std::fs::write(&entry, b"broken").unwrap();
    server.reset().await;
    cover(
        &server,
        "al-1",
        ResponseTemplate::new(200).set_body_bytes(jpeg(64)),
    )
    .await;
    assert!(matches!(
        load(&client, &cache, "al-1").await,
        Outcome::Art(_)
    ));
    assert!(image::load_from_memory(&std::fs::read(&entry).unwrap()).is_ok());
}

#[tokio::test]
async fn an_unreachable_server_is_a_failure_to_retry_not_no_art() {
    let client = Client::new(
        parse_base_url("http://127.0.0.1:9").unwrap(),
        USER.into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(DiskCache::new(dir.path().join("covers")));
    assert!(matches!(
        load(&client, &cache, "al-1").await,
        Outcome::Failed
    ));
    let error = client.cover_art("al-1", 256).await.unwrap_err();
    assert_secret_free(&error);
}

#[tokio::test]
async fn large_covers_are_asked_at_their_size_and_cached_apart() {
    let (server, client) = server().await;
    let dir = tempfile::tempdir().unwrap();
    let covers = dir.path().join("covers");
    let small = Arc::new(DiskCache::new(covers.clone()));
    let large = Arc::new(DiskCache::large(large_dir(&covers)));
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt"))
        .and(query_param("id", "al-1"))
        .and(query_param("size", "256"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(1200)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt"))
        .and(query_param("id", "al-1"))
        .and(query_param("size", "1024"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(1200)))
        .expect(2)
        .mount(&server)
        .await;
    let Outcome::Art(image) = load(&client, &small, "al-1").await else {
        panic!("small art expected");
    };
    assert_eq!(image.width(), COVER_SIZE);
    let Outcome::Art(image) = load_at(&client, &large, "al-1", CoverSize::Large).await else {
        panic!("large art expected");
    };
    assert_eq!(image.width(), LARGE_COVER_SIZE);
    // Each in its own folder, under the same secret-free name.
    let name = DiskCache::name(client.base_url().as_str(), USER, "al-1");
    assert!(covers.join(&name).is_file());
    assert!(covers.join("large").join(&name).is_file());
    // Both come from the disk the second time (the small mock expects one
    // request, the large one two: see below).
    let Outcome::Art(image) = load_at(&client, &large, "al-1", CoverSize::Large).await else {
        panic!("cached large art expected");
    };
    assert_eq!(image.width(), LARGE_COVER_SIZE);
    assert!(matches!(
        load(&client, &small, "al-1").await,
        Outcome::Art(_)
    ));
    // Handed the small cache by mistake, a large load does not answer
    // from it (nor store into it): it asks the server, the large mock's
    // second request.
    let Outcome::Art(image) = load_at(&client, &small, "al-1", CoverSize::Large).await else {
        panic!("art expected");
    };
    assert_eq!(image.width(), LARGE_COVER_SIZE);
    assert_eq!(small.get(&name).unwrap().width(), COVER_SIZE);
}
