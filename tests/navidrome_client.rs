//! Subsonic client against a local `wiremock` server. No test contacts a
//! real server.

use std::error::Error as _;
use std::time::Duration;

use coda_player::model::{MAX_METADATA_CHARS, TrackSource, UNKNOWN_TITLE};
use coda_player::navidrome::client::{self, MAX_BODY_BYTES, parse_base_url};
use coda_player::navidrome::{Client, Error, Password, UNKNOWN_ARTIST};
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
