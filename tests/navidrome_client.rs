//! Subsonic client against a local `wiremock` server. No test contacts a
//! real server.

use std::error::Error as _;
use std::time::Duration;

use coda_player::model::{MAX_METADATA_CHARS, TrackSource, UNKNOWN_TITLE};
use coda_player::navidrome::api::MAX_PLAYLIST_ENTRIES;
use coda_player::navidrome::client::{
    self, MAX_BODY_BYTES, MAX_PLAYLIST_BODY_BYTES, parse_base_url,
};
use coda_player::navidrome::{AlbumSort, Client, Error, Password, UNKNOWN_ARTIST};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PASSWORD: &str = "s3cret-pass";
const USER: &str = "listener";

fn client_at(base: &str) -> Client {
    Client::new(
        parse_base_url(base).unwrap(),
        USER.into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap()
}

async fn server() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = client_at(&server.uri());
    (server, client)
}

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1", "type": "navidrome" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn failed(code: i64, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": code, "message": message }
        }
    }))
}

async fn mount(server: &MockServer, endpoint: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/rest/{endpoint}")))
        .respond_with(response)
        .mount(server)
        .await;
}

fn query(url: &url::Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn param(q: &[(String, String)], key: &str) -> Option<String> {
    q.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[tokio::test]
async fn requests_carry_token_auth_with_a_fresh_salt_and_no_password() {
    let (server, client) = server().await;
    mount(&server, "ping", ok(json!({}))).await;
    client.ping().await.unwrap();
    client.ping().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let mut salts = Vec::new();
    for request in &requests {
        let q = query(&request.url);
        assert_eq!(param(&q, "u").as_deref(), Some(USER));
        assert_eq!(param(&q, "v").as_deref(), Some(client::API_VERSION));
        assert_eq!(param(&q, "c").as_deref(), Some("coda"));
        assert_eq!(param(&q, "f").as_deref(), Some("json"));
        assert_eq!(param(&q, "p"), None, "plaintext password parameter sent");
        let salt = param(&q, "s").expect("salt");
        assert!(salt.len() >= 6);
        let token = param(&q, "t").expect("token");
        assert_eq!(
            token,
            format!("{:x}", md5::compute(format!("{PASSWORD}{salt}")))
        );
        assert!(!request.url.as_str().contains(PASSWORD));
        salts.push(salt);
    }
    assert_ne!(salts[0], salts[1], "salt must be fresh per request");
}

#[tokio::test]
async fn ping_results() {
    let (server, client) = server().await;
    mount(&server, "ping", ok(json!({}))).await;
    assert!(client.ping().await.is_ok());

    for (code, expect_auth) in [(40, true), (41, true), (50, false)] {
        let (server, client) = self::server().await;
        mount(&server, "ping", failed(code, "nope")).await;
        let err = client.ping().await.unwrap_err();
        if expect_auth {
            assert!(matches!(err, Error::Auth), "{code}: {err:?}");
            assert_eq!(err.to_string(), "authentication failed");
        } else {
            assert!(
                matches!(&err, Error::Server { code: 50, message } if message == "nope"),
                "{err:?}"
            );
        }
    }
}

#[tokio::test]
async fn path_prefix_base_urls_join_below_the_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/navidrome/rest/ping"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    for base in [
        format!("{}/navidrome", server.uri()),
        format!("{}/navidrome/", server.uri()),
    ] {
        client_at(&base).ping().await.unwrap();
    }
}

#[tokio::test]
async fn get_artists_flattens_the_index_and_skips_bad_items() {
    let (server, client) = server().await;
    mount(
        &server,
        "getArtists",
        ok(json!({ "artists": { "ignoredArticles": "The", "index": [
            { "name": "A", "artist": [
                { "id": "ar1", "name": "ABBA", "albumCount": 9, "coverArt": "x" },
                { "id": "", "name": "empty id" },
                { "name": "no id" },
            ]},
            { "name": "Z", "artist": [
                { "id": "ar2", "name": "z".repeat(50_000), "albumCount": -3 },
                { "id": "ar3" },
            ]},
            { "name": "empty" },
        ]}})),
    )
    .await;
    let artists = client.artists().await.unwrap();
    let ids: Vec<&str> = artists.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["ar1", "ar2", "ar3"]);
    assert_eq!(artists[0].name, "ABBA");
    assert_eq!(artists[0].album_count, Some(9));
    assert_eq!(artists[1].name.chars().count(), MAX_METADATA_CHARS);
    assert_eq!(artists[1].album_count, None);
    assert_eq!(artists[2].name, UNKNOWN_ARTIST);
}

#[tokio::test]
async fn get_artist_and_get_album() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar 1"))
        .respond_with(ok(
            json!({ "artist": { "id": "ar 1", "name": "X", "album": [
                { "id": "al1", "name": "First", "artist": "X", "year": 1999,
                  "songCount": 2, "duration": 600 },
                { "id": "al2", "name": "Second", "duration": -1, "year": 1e40 },
                { "name": "no id" },
            ]}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(ok(
            json!({ "album": { "id": "al1", "name": "First", "song": [
                { "id": "s1", "title": "One", "artist": "X", "album": "First",
                  "track": 1, "discNumber": 1, "duration": 200, "suffix": "flac" },
                { "id": "s2", "duration": 1e300 },
                { "id": "", "title": "skipped" },
            ]}}),
        ))
        .mount(&server)
        .await;

    let albums = client.artist("ar 1").await.unwrap();
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].name, "First");
    assert_eq!(albums[0].year, Some(1999));
    assert_eq!(albums[0].song_count, Some(2));
    assert_eq!(albums[0].duration, Some(Duration::from_secs(600)));
    assert_eq!(albums[1].duration, None);
    assert_eq!(albums[1].year, None);

    let tracks = client.album("al1").await.unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(
        tracks[0].source,
        TrackSource::Navidrome {
            song_id: "s1".into()
        }
    );
    assert_eq!(tracks[0].title, "One");
    assert_eq!(tracks[0].track_no, Some(1));
    assert_eq!(tracks[0].duration, Some(Duration::from_secs(200)));
    assert_eq!(tracks[1].title, UNKNOWN_TITLE);
    assert_eq!(tracks[1].duration, None);
    assert!(tracks[1].is_remote());
}

#[tokio::test]
async fn ok_response_without_the_payload_is_a_decode_error() {
    let (server, client) = server().await;
    mount(&server, "getAlbum", ok(json!({}))).await;
    assert!(matches!(client.album("x").await, Err(Error::Decode(_))));
}

#[tokio::test]
async fn search3_parses_all_sections_with_bounded_counts() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "blue & \"moon\""))
        .and(query_param("artistCount", "20"))
        .and(query_param("albumCount", "20"))
        .and(query_param("songCount", "50"))
        .respond_with(ok(json!({ "searchResult3": {
            "artist": [{ "id": "a", "name": "Blue" }],
            "album": [{ "id": "b", "name": "Moon", "artist": "Blue" }, { "id": 7 }],
            "song": [{ "id": "c", "title": "Blue Moon", "duration": 3.5 }, "garbage"],
        }})))
        .mount(&server)
        .await;
    let results = client.search("blue & \"moon\"").await.unwrap();
    assert_eq!(results.artists.len(), 1);
    assert_eq!(results.albums.len(), 2);
    assert_eq!(results.albums[1].id, "7");
    assert_eq!(results.tracks.len(), 1);
    assert_eq!(
        results.tracks[0].duration,
        Some(Duration::from_millis(3500))
    );

    // No results section at all: empty, not an error.
    let (server, client) = self::server().await;
    mount(&server, "search3", ok(json!({}))).await;
    let results = client.search("x").await.unwrap();
    assert!(results.artists.is_empty() && results.albums.is_empty() && results.tracks.is_empty());
}

#[tokio::test]
async fn http_errors_and_bad_bodies() {
    let (server, client) = server().await;
    mount(&server, "ping", ResponseTemplate::new(500)).await;
    let err = client.ping().await.unwrap_err();
    assert!(matches!(err, Error::Http(500)), "{err:?}");

    let (server, client) = self::server().await;
    mount(
        &server,
        "ping",
        ResponseTemplate::new(200).set_body_string("<html>not json</html>"),
    )
    .await;
    let err = client.ping().await.unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
    assert_eq!(err.to_string(), "unexpected response from server");
    let detail = err.detail();
    assert!(detail.starts_with("decode: ping:"), "{detail}");
    assert!(!detail.contains("html"), "response data in {detail}");

    let (server, client) = self::server().await;
    mount(
        &server,
        "ping",
        ResponseTemplate::new(200).set_body_json(json!({ "something": "else" })),
    )
    .await;
    assert!(matches!(client.ping().await, Err(Error::Decode(_))));

    let (server, client) = self::server().await;
    mount(
        &server,
        "ping",
        ResponseTemplate::new(200)
            .set_body_json(json!({ "subsonic-response": { "status": "maybe" } })),
    )
    .await;
    assert!(matches!(client.ping().await, Err(Error::Decode(_))));
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let (server, client) = server().await;
    mount(
        &server,
        "ping",
        ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1:9/elsewhere"),
    )
    .await;
    let err = client.ping().await.unwrap_err();
    assert!(matches!(err, Error::Redirect(302)), "{err:?}");
    assert!(err.to_string().contains("check the url"), "{err}");
    assert_eq!(err.detail(), "HTTP status 302");
    // Only the one request: the redirect target was never contacted.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bodies_over_the_cap_are_rejected() {
    let (server, client) = server().await;
    let huge = format!(
        r#"{{"subsonic-response":{{"status":"ok","pad":"{}"}}}}"#,
        "x".repeat(MAX_BODY_BYTES)
    );
    mount(
        &server,
        "ping",
        ResponseTemplate::new(200).set_body_raw(huge, "application/json"),
    )
    .await;
    let err = client.ping().await.unwrap_err();
    assert!(matches!(err, Error::TooLarge), "{err:?}");
}

#[tokio::test]
async fn slow_server_times_out() {
    let server = MockServer::start().await;
    mount(
        &server,
        "ping",
        ok(json!({})).set_delay(Duration::from_secs(5)),
    )
    .await;
    let client = Client::with_timeouts(
        parse_base_url(&server.uri()).unwrap(),
        USER.into(),
        Password::new(PASSWORD.into()),
        Duration::from_secs(1),
        Duration::from_millis(200),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let err = client.ping().await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        matches!(&err, Error::Network(e) if e.is_timeout()),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "server did not respond in time");
    assert_eq!(err.detail(), "network error: timeout");
    assert_secret_free(&err);
}

/// Display, Debug and every source in the chain are free of the password,
/// username, auth parameters and the query string.
fn assert_secret_free(err: &Error) {
    let mut texts = vec![err.to_string(), format!("{err:?}")];
    let mut source = err.source();
    while let Some(s) = source {
        texts.push(s.to_string());
        texts.push(format!("{s:?}"));
        source = s.source();
    }
    texts.push(err.detail());
    for text in texts {
        for needle in [PASSWORD, USER, "t=", "s=", "u=", "v=1.16.1", "rest/", "?"] {
            assert!(!text.contains(needle), "{needle:?} leaked in {text:?}");
        }
    }
}

#[tokio::test]
async fn connection_errors_contain_no_secrets() {
    // A port that was just free: connection refused.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let client = client_at(&format!("http://127.0.0.1:{port}/prefix"));
    for result in [
        client.ping().await,
        client.artists().await.map(drop),
        client.search("query").await.map(drop),
    ] {
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
        assert_eq!(err.to_string(), "could not reach server");
        assert_eq!(
            err.detail(),
            "network error: connect (including DNS and TLS)"
        );
        assert_secret_free(&err);
    }
}

#[tokio::test]
async fn debug_output_of_client_and_stream_url_is_redacted() {
    let (_server, client) = server().await;
    let text = format!("{client:?}");
    assert!(!text.contains(PASSWORD) && !text.contains(USER) && !text.contains("127.0.0.1"));
    let stream = client.stream_url("song").unwrap();
    for text in [format!("{stream:?}"), stream.to_string()] {
        assert!(!text.contains(PASSWORD) && !text.contains("song") && !text.contains("t="));
    }
    let password = Password::new(PASSWORD.into());
    assert!(!format!("{password:?}").contains(PASSWORD));
    assert!(!password.to_string().contains(PASSWORD));
}

/// Serves one response on a raw socket: a chunked body larger than the cap
/// (no Content-Length), so the client must stop while reading chunks.
#[tokio::test]
async fn chunked_bodies_over_the_cap_are_rejected_while_reading() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Read the request head.
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed before sending a request");
            head.extend_from_slice(&buf[..n]);
        }
        let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                      Transfer-Encoding: chunked\r\n\r\n";
        socket.write_all(header.as_bytes()).await.unwrap();
        let chunk = vec![b' '; 64 * 1024];
        let mut sent = 0;
        // Up to twice the cap; stop early once the client hangs up.
        while sent <= 2 * MAX_BODY_BYTES {
            let frame = format!("{:x}\r\n", chunk.len());
            if socket.write_all(frame.as_bytes()).await.is_err()
                || socket.write_all(&chunk).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return sent;
            }
            sent += chunk.len();
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
        sent
    });
    let client = client_at(&format!("http://{addr}"));
    let err = client.ping().await.unwrap_err();
    assert!(matches!(err, Error::TooLarge), "{err:?}");
    let sent = tokio::time::timeout(Duration::from_secs(10), served)
        .await
        .expect("server finishes")
        .unwrap();
    assert!(sent > MAX_BODY_BYTES, "only {sent} bytes were sent");
}

#[tokio::test]
async fn scrobble_sends_id_submission_and_time() {
    let (server, client) = server().await;
    mount(&server, "scrobble", ok(json!({}))).await;
    client.scrobble("song 1&x", false, None).await.unwrap();
    let played = std::time::UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    client.scrobble("s2", true, Some(played)).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let now_playing = query(&requests[0].url);
    assert_eq!(param(&now_playing, "id").as_deref(), Some("song 1&x"));
    assert_eq!(param(&now_playing, "submission").as_deref(), Some("false"));
    assert_eq!(param(&now_playing, "time"), None);
    assert!(
        param(&now_playing, "t").is_some(),
        "signed like every request"
    );
    let submission = query(&requests[1].url);
    assert_eq!(param(&submission, "id").as_deref(), Some("s2"));
    assert_eq!(param(&submission, "submission").as_deref(), Some("true"));
    assert_eq!(param(&submission, "time").as_deref(), Some("1700000000123"));
    // A failed status is an error like for every endpoint.
    let (server, client) = self::server().await;
    mount(&server, "scrobble", failed(70, "not found")).await;
    let err = client.scrobble("x", true, None).await.unwrap_err();
    assert!(matches!(err, Error::Server { code: 70, .. }), "{err:?}");
}

#[tokio::test]
async fn scrobbler_sends_in_order_and_reports_only_the_first_failure() {
    use coda_player::navidrome::scrobble::{ScrobbleRequest, Scrobbler};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/scrobble"))
        .and(query_param("id", "bad"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    mount(&server, "scrobble", ok(json!({}))).await;
    let (scrobbler, mut failure) = Scrobbler::spawn(client_at(&server.uri()));
    let started = std::time::UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    scrobbler.send(ScrobbleRequest::now_playing("a".into()));
    scrobbler.send(ScrobbleRequest {
        song_id: "a".into(),
        submission: true,
        started_at: Some(started),
    });
    scrobbler.send(ScrobbleRequest::now_playing("bad".into()));
    scrobbler.send(ScrobbleRequest::now_playing("bad".into()));
    scrobbler.send(ScrobbleRequest::now_playing("c".into()));
    let error = tokio::time::timeout(Duration::from_secs(5), &mut failure)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(error, Error::Http(500)), "{error:?}");
    // Wait for the rest, then check order and parameters.
    let mut requests = Vec::new();
    for _ in 0..100 {
        requests = server.received_requests().await.unwrap();
        if requests.len() == 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let sent: Vec<(String, String, Option<String>)> = requests
        .iter()
        .map(|r| {
            let q = query(&r.url);
            (
                param(&q, "id").unwrap(),
                param(&q, "submission").unwrap(),
                param(&q, "time"),
            )
        })
        .collect();
    let ids: Vec<(&str, &str)> = sent
        .iter()
        .map(|(id, sub, _)| (id.as_str(), sub.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("a", "false"),
            ("a", "true"),
            ("bad", "false"),
            ("bad", "false"),
            ("c", "false")
        ]
    );
    // The submission carries the start of the play.
    assert_eq!(sent[1].2.as_deref(), Some("1700000000123"));
    assert!(
        sent.iter()
            .filter(|s| s.1 == "false")
            .all(|s| s.2.is_none())
    );
}

#[tokio::test]
async fn scrobbler_drops_requests_beyond_its_queue() {
    use coda_player::navidrome::scrobble::{QUEUE_CAPACITY, ScrobbleRequest, Scrobbler};

    let server = MockServer::start().await;
    // Slow enough that every request below is sent while the first one is
    // still running, so the queue fills up.
    mount(
        &server,
        "scrobble",
        ok(json!({})).set_delay(Duration::from_millis(300)),
    )
    .await;
    let (scrobbler, _failure) = Scrobbler::spawn(client_at(&server.uri()));
    // The first request reaches the server and holds the worker busy...
    scrobbler.send(ScrobbleRequest::now_playing("0".into()));
    let received = || async { server.received_requests().await.unwrap().len() };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while received().await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first request never reached the server"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // ...so the queue holds exactly `QUEUE_CAPACITY` of the rest.
    let total = QUEUE_CAPACITY + 50;
    for i in 1..total {
        // Never blocks, however many are sent.
        scrobbler.send(ScrobbleRequest::now_playing(i.to_string()));
    }
    // Wait until everything the queue held has arrived (each request
    // takes 300 ms), then two more request times, in which anything sent
    // beyond the queue's capacity would have arrived too.
    let expected_count = QUEUE_CAPACITY + 1;
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(300 * (expected_count as u64)) * 4
        + Duration::from_secs(5);
    while received().await < expected_count && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    let ids: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| param(&query(&r.url), "id").unwrap())
        .collect();
    // In order, and nothing beyond what the queue could hold.
    let expected: Vec<String> = (0..=QUEUE_CAPACITY).map(|i| i.to_string()).collect();
    assert_eq!(ids, expected, "{} of {total} sent", ids.len());
}

// ----- M5: album lists and playlists -------------------------------------

fn assert_signed(request: &wiremock::Request) {
    let q = query(&request.url);
    assert_eq!(param(&q, "u").as_deref(), Some(USER));
    assert_eq!(param(&q, "f").as_deref(), Some("json"));
    assert_eq!(param(&q, "p"), None, "plaintext password parameter sent");
    let salt = param(&q, "s").expect("salt");
    assert_eq!(
        param(&q, "t").as_deref(),
        Some(format!("{:x}", md5::compute(format!("{PASSWORD}{salt}"))).as_str())
    );
    assert!(!request.url.as_str().contains(PASSWORD));
}

#[tokio::test]
async fn album_list_sends_the_sort_size_and_offset() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2"))
        .and(query_param("type", "alphabeticalByArtist"))
        .and(query_param("size", "500"))
        .and(query_param("offset", "1500"))
        .respond_with(ok(json!({ "albumList2": { "album": [
            { "id": "al1", "name": "First", "artist": "X", "year": 1999,
              "songCount": 2, "duration": 600 },
            { "id": "al2", "name": "z".repeat(50_000), "songCount": 1e20,
              "duration": 1e300, "year": -1 },
            { "name": "no id" },
            "garbage",
        ]}})))
        .mount(&server)
        .await;
    let page = client
        .album_list(AlbumSort::Artist, 1500, 500)
        .await
        .unwrap();
    assert_eq!(page.served, 4, "unusable items still count");
    assert_eq!(page.albums.len(), 2);
    assert_eq!(page.albums[0].name, "First");
    assert_eq!(page.albums[0].artist.as_deref(), Some("X"));
    assert_eq!(page.albums[1].name.chars().count(), MAX_METADATA_CHARS);
    assert_eq!(page.albums[1].song_count, Some(u32::MAX));
    assert_eq!(page.albums[1].duration, Some(Duration::MAX));
    assert_eq!(page.albums[1].year, None);
    assert_signed(&server.received_requests().await.unwrap()[0]);
}

#[tokio::test]
async fn album_list_types_come_from_the_closed_set_and_size_is_clamped() {
    let (server, client) = server().await;
    mount(&server, "getAlbumList2", ok(json!({ "albumList2": {} }))).await;
    for (sort, size) in AlbumSort::ALL
        .into_iter()
        .zip([0, 1, 499, 500, 501, usize::MAX])
    {
        let page = client.album_list(sort, 0, size).await.unwrap();
        // Subsonic omits the array of an empty list.
        assert_eq!(page, Default::default());
    }
    let requests = server.received_requests().await.unwrap();
    let sent: Vec<(String, String)> = requests
        .iter()
        .map(|r| {
            let q = query(&r.url);
            (param(&q, "type").unwrap(), param(&q, "size").unwrap())
        })
        .collect();
    let expected = [
        ("alphabeticalByName", "1"),
        ("alphabeticalByArtist", "1"),
        ("newest", "499"),
        ("frequent", "500"),
        ("recent", "500"),
        ("random", "500"),
    ];
    assert_eq!(
        sent,
        expected.map(|(t, s)| (t.to_owned(), s.to_owned())).to_vec()
    );
    // More items than asked for: only the size counts.
    let (server, client) = self::server().await;
    let items: Vec<Value> = (0..30).map(|i| json!({ "id": format!("a{i}") })).collect();
    mount(
        &server,
        "getAlbumList2",
        ok(json!({ "albumList2": { "album": items } })),
    )
    .await;
    let page = client.album_list(AlbumSort::Random, 0, 10).await.unwrap();
    assert_eq!((page.served, page.albums.len()), (10, 10));
}

#[tokio::test]
async fn playlists_and_playlist_entries_in_order_with_duplicates() {
    let (server, client) = server().await;
    mount(
        &server,
        "getPlaylists",
        ok(json!({ "playlists": { "playlist": [
            { "id": "p1", "name": "Road Trip", "songCount": 3, "duration": 750,
              "comment": "c".repeat(100_000), "owner": "me", "public": false },
            { "id": "p2", "name": "", "songCount": 99_999_999_999u64, "duration": -3 },
            { "name": "no id", "songCount": 1 },
        ]}})),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylist"))
        .and(query_param("id", "p 1&x"))
        .respond_with(ok(
            json!({ "playlist": { "id": "p 1&x", "name": "Road Trip", "entry": [
                { "id": "s1", "title": "One", "artist": "A", "duration": 200 },
                { "id": "s2", "title": "Two" },
                { "title": "no id" },
                { "id": "s1", "title": "One", "artist": "A", "duration": 200 },
            ]}}),
        ))
        .mount(&server)
        .await;
    let playlists = client.playlists().await.unwrap();
    assert_eq!(playlists.len(), 2);
    assert_eq!(playlists[0].name, "Road Trip");
    assert_eq!(playlists[0].song_count, Some(3));
    assert_eq!(playlists[0].duration, Some(Duration::from_secs(750)));
    assert_eq!(playlists[1].name, "Unnamed playlist");
    assert_eq!(playlists[1].song_count, Some(u32::MAX));
    assert_eq!(playlists[1].duration, None);
    let songs = client.playlist("p 1&x").await.unwrap();
    assert_eq!(songs.served, 4);
    assert_eq!(songs.truncated(), None);
    let ids: Vec<_> = songs
        .tracks
        .iter()
        .map(|t| match &t.source {
            TrackSource::Navidrome { song_id } => song_id.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["s1", "s2", "s1"]);
    for request in server.received_requests().await.unwrap() {
        assert_signed(&request);
    }

    // No playlists at all: Subsonic sends an empty object.
    let (server, client) = self::server().await;
    mount(&server, "getPlaylists", ok(json!({ "playlists": {} }))).await;
    mount(
        &server,
        "getPlaylist",
        ok(json!({ "playlist": { "id": "e", "name": "Empty" } })),
    )
    .await;
    assert!(client.playlists().await.unwrap().is_empty());
    assert!(client.playlist("e").await.unwrap().tracks.is_empty());
}

#[tokio::test]
async fn new_calls_report_failed_status_and_missing_sections() {
    let (server, client) = server().await;
    for endpoint in ["getAlbumList2", "getPlaylists", "getPlaylist"] {
        mount(&server, endpoint, failed(70, "not found")).await;
    }
    let errors = [
        client
            .album_list(AlbumSort::Name, 0, 500)
            .await
            .unwrap_err(),
        client.playlists().await.unwrap_err(),
        client.playlist("x").await.unwrap_err(),
    ];
    for err in errors {
        assert!(
            matches!(&err, Error::Server { code: 70, message } if message == "not found"),
            "{err:?}"
        );
    }
    let (server, client) = self::server().await;
    for endpoint in ["getAlbumList2", "getPlaylists", "getPlaylist"] {
        mount(&server, endpoint, ok(json!({ "playlist": "nope" }))).await;
    }
    assert!(matches!(
        client.album_list(AlbumSort::Name, 0, 500).await,
        Err(Error::Decode(_))
    ));
    assert!(matches!(client.playlists().await, Err(Error::Decode(_))));
    assert!(matches!(client.playlist("x").await, Err(Error::Decode(_))));
    let (server, client) = self::server().await;
    mount(
        &server,
        "getPlaylists",
        failed(40, "Wrong username or password"),
    )
    .await;
    assert!(matches!(client.playlists().await, Err(Error::Auth)));
}

/// An OpenSubsonic entry as Navidrome sends it in `getPlaylist`, with
/// several artists, contributors and a long path: about 2 KB.
fn navidrome_entry(i: usize) -> Value {
    let artists: Vec<Value> = (0..3)
        .map(|k| json!({ "id": format!("7cD3eF5gH7iJ9kL1mN3o{k:02}"), "name": format!("Example Artist Number {k}") }))
        .collect();
    // In parts: one `json!` this size exceeds the macro recursion limit.
    let mut entry = json!({
        "id": format!("4Kdq9iW1nGfmXxyzQab{i:05}"), "parent": "2bA0lQ8sVnLkPpRtYu1x",
        "isDir": false,
        "title": format!("Some Rather Long Song Title Number {i} (Live at the Example Hall, 1999 Remaster)"),
        "album": "A Typical Album Name (Deluxe Edition, Remastered 2024)",
        "artist": "Example Artist Number 0 feat. Example Artist Number 1 & Example Artist Number 2",
        "track": 3, "year": 1999, "genre": "Rock", "coverArt": "mf-4Kdq9iW1nGfmXxyzQab_6512a3f1",
        "size": 38_456_123, "contentType": "audio/flac", "suffix": "flac", "duration": 245,
        "bitRate": 1011,
    });
    let more = json!({
        "path": format!("Example Artist Number 0/A Typical Album Name (Deluxe Edition, Remastered 2024)/Disc 1/03 - Some Rather Long Song Title Number {i} (Live at the Example Hall, 1999 Remaster).flac"),
        "playCount": 3, "played": "2024-05-01T12:34:56.123Z", "discNumber": 1,
        "created": "2023-02-03T04:05:06.789Z", "albumId": "2bA0lQ8sVnLkPpRtYu1x",
        "artistId": "7cD3eF5gH7iJ9kL1mN3o00", "type": "music", "isVideo": false, "bpm": 0,
        "comment": "", "sortName": "some rather long song title", "mediaType": "song",
        "musicBrainzId": "5f3a1c2e-1234-4b5c-8d9e-0a1b2c3d4e5f",
        "genres": [{ "name": "Rock" }, { "name": "Live" }],
    });
    let rest = json!({
        "replayGain": { "trackGain": -7.5, "albumGain": -7.1, "trackPeak": 0.98, "albumPeak": 1 },
        "channelCount": 2, "samplingRate": 44100, "bitDepth": 16, "moods": ["energetic"],
        "artists": artists.clone(),
        "displayArtist": "Example Artist Number 0 feat. Example Artist Number 1 & Example Artist Number 2",
        "albumArtists": [artists[0].clone()], "displayAlbumArtist": "Example Artist Number 0",
        "contributors": [
            { "role": "composer", "artist": artists[1].clone() },
            { "role": "producer", "artist": artists[2].clone() },
            { "role": "performer", "subRole": "guitar", "artist": artists[0].clone() },
        ],
        "displayComposer": "Example Artist Number 1", "explicitStatus": "", "isrc": ["USRC17607839"],
    });
    for part in [more, rest] {
        if let (Some(entry), Value::Object(part)) = (entry.as_object_mut(), part) {
            entry.extend(part);
        }
    }
    entry
}

#[tokio::test]
async fn a_playlist_of_10000_realistic_entries_parses_within_its_cap() {
    let (server, client) = server().await;
    let entries: Vec<Value> = (0..10_000).map(navidrome_entry).collect();
    let body = serde_json::to_vec(&json!({ "subsonic-response": {
        "status": "ok", "version": "1.16.1", "type": "navidrome", "openSubsonic": true,
        "playlist": { "id": "big", "name": "Big", "songCount": 10_000, "entry": entries }
    }}))
    .unwrap();
    // Measured: over the general cap, within the playlist cap.
    assert!(body.len() > MAX_BODY_BYTES, "{} bytes", body.len());
    assert!(body.len() < MAX_PLAYLIST_BODY_BYTES, "{} bytes", body.len());
    mount(
        &server,
        "getPlaylist",
        ResponseTemplate::new(200).set_body_raw(body, "application/json"),
    )
    .await;
    let songs = client.playlist("big").await.unwrap();
    assert_eq!(songs.tracks.len(), 10_000);
    assert_eq!(songs.truncated(), None);
    assert!(
        songs.tracks[9_999]
            .title
            .starts_with("Some Rather Long Song Title Number 9999")
    );
    assert_eq!(songs.tracks[0].duration, Some(Duration::from_secs(245)));
}

#[tokio::test]
async fn playlist_entries_beyond_the_cap_are_skipped_and_huge_bodies_rejected() {
    let (server, client) = server().await;
    let entries: Vec<Value> = (0..MAX_PLAYLIST_ENTRIES + 5)
        .map(|i| json!({ "id": i.to_string() }))
        .collect();
    mount(
        &server,
        "getPlaylist",
        ok(json!({ "playlist": { "id": "p", "entry": entries } })),
    )
    .await;
    let songs = client.playlist("p").await.unwrap();
    assert_eq!(songs.tracks.len(), MAX_PLAYLIST_ENTRIES);
    assert_eq!(songs.served, MAX_PLAYLIST_ENTRIES + 5);
    assert_eq!(
        songs.truncated(),
        Some((MAX_PLAYLIST_ENTRIES, MAX_PLAYLIST_ENTRIES + 5))
    );
    assert_eq!(songs.truncation_note(), " (first 10,000 of 10,005)");
    let (server, client) = self::server().await;
    let huge = format!(
        r#"{{"subsonic-response":{{"status":"ok","pad":"{}"}}}}"#,
        "x".repeat(MAX_PLAYLIST_BODY_BYTES)
    );
    mount(
        &server,
        "getPlaylist",
        ResponseTemplate::new(200).set_body_raw(huge, "application/json"),
    )
    .await;
    assert!(matches!(client.playlist("p").await, Err(Error::TooLarge)));
}

/// A body just under the playlist cap whose single entry holds a huge
/// nested array, in a known field or under an unknown key. Parsing drains
/// it without building it (measured: about 22 MB peak RSS for a 16 MB
/// body, where building it took about 1.5 GB), so this stays fast.
#[tokio::test]
async fn a_single_hostile_entry_at_the_cap_is_drained() {
    for key in ["title", "notAField"] {
        let (server, client) = server().await;
        let head = format!(
            r#"{{"subsonic-response":{{"status":"ok","playlist":{{"id":"p","entry":[{{"id":"s1","{key}":["#
        );
        let tail = r#"{"a":0}]},{"id":"s2","title":"Fine"}]}}}"#;
        let item = r#"{"a":0},"#;
        let n = (MAX_PLAYLIST_BODY_BYTES - head.len() - tail.len() - 16) / item.len();
        let mut body = String::with_capacity(MAX_PLAYLIST_BODY_BYTES);
        body.push_str(&head);
        for _ in 0..n {
            body.push_str(item);
        }
        body.push_str(tail);
        assert!(body.len() <= MAX_PLAYLIST_BODY_BYTES);
        mount(
            &server,
            "getPlaylist",
            ResponseTemplate::new(200).set_body_raw(body, "application/json"),
        )
        .await;
        let started = std::time::Instant::now();
        let songs = client.playlist("p").await.unwrap();
        assert_eq!(songs.served, 2, "{key}");
        assert_eq!(songs.tracks[1].title, "Fine");
        assert!(started.elapsed() < Duration::from_secs(10), "{key}");
    }
}

/// A body of about `bytes` whose `artists` section nests empty objects
/// four deep (index → artist → album → song), plus `extra` sections.
fn nested_empty_objects(bytes: usize, extra: &str) -> String {
    let chunk = r#"{"artist":[{"album":[{"song":[{},{},{}]},{}]},{}]},"#;
    let head = format!(r#"{{"subsonic-response":{{"status":"ok",{extra}"artists":{{"index":["#);
    let tail = "{}]}}}";
    let n = (bytes - head.len() - tail.len()) / chunk.len();
    let mut body = String::with_capacity(bytes);
    body.push_str(&head);
    for _ in 0..n {
        body.push_str(chunk);
    }
    body.push_str(tail);
    body
}

/// Nested `{}` at each body cap, through the real client path: the
/// response budget (and, for `getPlaylist`, draining every section but its
/// own) keeps this bounded. Measured peak RSS: see ARCHITECTURE.md.
#[tokio::test]
async fn nested_empty_objects_at_each_cap_parse_in_bounded_time() {
    let (server, client) = server().await;
    let artists = nested_empty_objects(MAX_BODY_BYTES - 64, "");
    mount(
        &server,
        "getArtists",
        ResponseTemplate::new(200).set_body_raw(artists, "application/json"),
    )
    .await;
    let playlist = nested_empty_objects(
        MAX_PLAYLIST_BODY_BYTES - 64,
        r#""playlist":{"id":"p","entry":[{"id":"s1"},{}]},"#,
    );
    mount(
        &server,
        "getPlaylist",
        ResponseTemplate::new(200).set_body_raw(playlist, "application/json"),
    )
    .await;
    let started = std::time::Instant::now();
    assert!(client.artists().await.unwrap().is_empty(), "no ids");
    let songs = client.playlist("p").await.unwrap();
    assert_eq!((songs.served, songs.tracks.len()), (2, 1));
    assert!(started.elapsed() < Duration::from_secs(20));
}
