//! Navidrome lyrics (`Client::lyrics`) against a local `wiremock` server.
//! No test contacts a real server.

use std::time::Duration;

use coda_core::lyrics::{Line, MAX_LINE_CHARS, MAX_LINES};
use coda_core::navidrome::client::{MAX_BODY_BYTES, parse_base_url};
use coda_core::navidrome::{Client, Error, Password};
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const PASSWORD: &str = "s3cret-pass";

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
    Mock::given(path(format!("/rest/{endpoint}")))
        .respond_with(response)
        .mount(server)
        .await;
}

/// A server that lists `songLyrics` (as current Navidrome does) or has no
/// OpenSubsonic extensions at all.
async fn start(song_lyrics: bool) -> (MockServer, Client) {
    let server = MockServer::start().await;
    let extensions = if song_lyrics {
        ok(json!({ "openSubsonicExtensions": [
            { "name": "formPost", "versions": [1] },
            { "name": "songLyrics", "versions": [1] }
        ]}))
    } else {
        ResponseTemplate::new(404)
    };
    mount(&server, "getOpenSubsonicExtensions", extensions).await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    (server, client)
}

async fn sent(server: &MockServer, endpoint: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == format!("/rest/{endpoint}"))
        .collect()
}

fn param(request: &Request, key: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn timed(ms: u64, text: &str) -> Line {
    Line {
        start: Some(Duration::from_millis(ms)),
        text: text.to_owned(),
    }
}

#[tokio::test]
async fn song_lyrics_prefer_the_synced_entry_with_its_offset() {
    let (server, client) = start(true).await;
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "lang": "eng", "synced": false, "line": [{ "value": "plain" }] },
            "not an object",
            {
                "displayArtist": "A", "lang": "xxx", "synced": true, "offset": 100,
                "line": [
                    { "start": 2000, "value": "second" },
                    { "start": 50, "value": "first" },
                    { "start": 3000.7, "value": "" },
                    { "value": "no start" },
                    { "start": -5, "value": "negative" },
                    { "start": 4000, "value": { "nested": ["junk"] } }
                ]
            }
        ]}})),
    )
    .await;
    let lyrics = client
        .lyrics("song-1", Some("Artist"), "Title")
        .await
        .unwrap()
        .unwrap();
    assert!(lyrics.timed);
    assert_eq!(
        lyrics.lines,
        [
            timed(0, "first"),
            timed(1_900, "second"),
            timed(2_900, ""),
            timed(3_900, "")
        ]
    );
    let requests = sent(&server, "getLyricsBySongId").await;
    assert_eq!(requests.len(), 1);
    assert_eq!(param(&requests[0], "id").as_deref(), Some("song-1"));
    assert!(sent(&server, "getLyrics").await.is_empty());

    // The extensions are asked once per client.
    client.lyrics("song-1", None, "Title").await.unwrap();
    assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 1);
}

#[tokio::test]
async fn song_lyrics_without_a_synced_entry_are_untimed() {
    let (server, client) = start(true).await;
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": false, "line": [] },
            { "synced": false, "offset": 500, "line": [
                { "value": "" }, { "value": "one" }, { "value": "" }, { "start": 10, "value": "two" }, { "value": "  " }
            ]}
        ]}})),
    )
    .await;
    let lyrics = client.lyrics("s", None, "T").await.unwrap().unwrap();
    assert!(!lyrics.timed);
    let texts: Vec<_> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "", "two"]);
    assert!(lyrics.lines.iter().all(|l| l.start.is_none()));
}

#[tokio::test]
async fn an_empty_lyrics_list_is_no_lyrics() {
    for body in [
        json!({ "lyricsList": {} }),
        json!({ "lyricsList": { "structuredLyrics": [] } }),
        json!({ "lyricsList": { "structuredLyrics": [{ "synced": true, "line": [{ "start": 0, "value": "" }] }] } }),
        json!({}),
    ] {
        let (server, client) = start(true).await;
        mount(&server, "getLyricsBySongId", ok(body.clone())).await;
        assert_eq!(
            client.lyrics("s", Some("A"), "T").await.unwrap(),
            None,
            "{body}"
        );
        // The classic endpoint is not asked as well.
        assert!(sent(&server, "getLyrics").await.is_empty());
    }
}

#[tokio::test]
async fn without_song_lyrics_the_classic_endpoint_is_asked_by_artist_and_title() {
    let (server, client) = start(false).await;
    mount(
        &server,
        "getLyrics",
        ok(json!({ "lyrics": {
            "artist": "Artist", "title": "Title",
            "value": "\r\nFirst line\r\nSecond line\r\n\r\n"
        }})),
    )
    .await;
    let lyrics = client
        .lyrics("s", Some("Artist & Co"), "Title?")
        .await
        .unwrap()
        .unwrap();
    assert!(!lyrics.timed);
    let texts: Vec<_> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["First line", "Second line"]);
    let requests = sent(&server, "getLyrics").await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        param(&requests[0], "artist").as_deref(),
        Some("Artist & Co")
    );
    assert_eq!(param(&requests[0], "title").as_deref(), Some("Title?"));
    assert!(sent(&server, "getLyricsBySongId").await.is_empty());
}

#[tokio::test]
async fn the_classic_endpoint_is_not_asked_without_an_artist() {
    let (server, client) = start(false).await;
    mount(
        &server,
        "getLyrics",
        ok(json!({ "lyrics": { "value": "x" } })),
    )
    .await;
    assert_eq!(client.lyrics("s", None, "T").await.unwrap(), None);
    assert_eq!(client.lyrics("s", Some("  "), "T").await.unwrap(), None);
    assert!(sent(&server, "getLyrics").await.is_empty());
}

#[tokio::test]
async fn classic_lyrics_that_are_missing_or_lrc() {
    let (server, client) = start(false).await;
    mount(&server, "getLyrics", ok(json!({ "lyrics": {} }))).await;
    assert_eq!(client.lyrics("s", Some("A"), "T").await.unwrap(), None);

    let (server, client) = start(false).await;
    mount(
        &server,
        "getLyrics",
        ok(json!({ "lyrics": { "value": "[00:01.00]one\n[00:02.00]two" } })),
    )
    .await;
    let lyrics = client.lyrics("s", Some("A"), "T").await.unwrap().unwrap();
    assert_eq!(lyrics.lines, [timed(1_000, "one"), timed(2_000, "two")]);
}

#[tokio::test]
async fn subsonic_errors() {
    // Not found is no lyrics.
    let (server, client) = start(true).await;
    mount(&server, "getLyricsBySongId", failed(70, "not found")).await;
    assert_eq!(client.lyrics("s", None, "T").await.unwrap(), None);
    let (server, client) = start(false).await;
    mount(&server, "getLyrics", failed(70, "not found")).await;
    assert_eq!(client.lyrics("s", Some("A"), "T").await.unwrap(), None);

    // Other errors are errors, never "no lyrics".
    let (server, client) = start(true).await;
    mount(&server, "getLyricsBySongId", failed(0, "boom")).await;
    let err = client.lyrics("s", None, "T").await.unwrap_err();
    assert!(matches!(err, Error::Server { code: 0, .. }), "{err:?}");
    let (server, client) = start(true).await;
    mount(&server, "getLyricsBySongId", failed(40, "wrong password")).await;
    let err = client.lyrics("s", None, "T").await.unwrap_err();
    assert!(matches!(err, Error::Auth), "{err:?}");
    let (server, client) = start(false).await;
    mount(&server, "getLyrics", ResponseTemplate::new(500)).await;
    let err = client.lyrics("s", Some("A"), "T").await.unwrap_err();
    assert!(matches!(err, Error::Http(500)), "{err:?}");
    let message = format!("{err} {err:?} {}", err.detail());
    assert!(
        !message.contains(PASSWORD) && !message.contains("127.0.0.1"),
        "{message}"
    );
}

#[tokio::test]
async fn an_uncertain_extension_answer_fails_and_is_asked_again() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getOpenSubsonicExtensions"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({ "openSubsonicExtensions": [{ "name": "songLyrics", "versions": [1] }] })),
    )
    .await;
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": true, "line": [{ "start": 0, "value": "hi" }] }
        ]}})),
    )
    .await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    assert!(matches!(
        client.lyrics("s", None, "T").await,
        Err(Error::Http(503))
    ));
    let lyrics = client.lyrics("s", None, "T").await.unwrap().unwrap();
    assert_eq!(lyrics.lines, [timed(0, "hi")]);
    assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 2);
}

#[tokio::test]
async fn oversized_bodies_are_refused_and_long_replies_capped() {
    // A body over the cap is refused.
    let (server, client) = start(true).await;
    let huge = "x".repeat(MAX_BODY_BYTES);
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": false, "line": [{ "value": huge }] }
        ]}})),
    )
    .await;
    let err = client.lyrics("s", None, "T").await.unwrap_err();
    assert!(matches!(err, Error::TooLarge), "{err:?}");

    // Within the cap, lines and their text are capped.
    let (server, client) = start(true).await;
    let lines: Vec<Value> = (0..MAX_LINES as u64 + 50)
        .map(|i| json!({ "start": i * 10, "value": "é".repeat(MAX_LINE_CHARS + 10) }))
        .collect();
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [{ "synced": true, "line": lines }] } })),
    )
    .await;
    let lyrics = client.lyrics("s", None, "T").await.unwrap().unwrap();
    assert_eq!(lyrics.lines.len(), MAX_LINES);
    assert!(
        lyrics
            .lines
            .iter()
            .all(|l| l.text.chars().count() == MAX_LINE_CHARS)
    );
}

#[tokio::test]
async fn lyrics_and_writes_share_one_extension_detection() {
    let (server, client) = start(true).await;
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": {} })),
    )
    .await;
    mount(&server, "updatePlaylist", ok(json!({}))).await;
    // A clone shares the answer too.
    let other = client.clone();
    assert_eq!(client.lyrics("s", None, "T").await.unwrap(), None);
    other.rename_playlist("p", "new name").await.unwrap();
    assert_eq!(
        other.transport().await.unwrap(),
        coda_core::navidrome::client::Transport::FormPost
    );
    assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 1);
    // The write went as a form POST, so the detection was used for both.
    let writes = sent(&server, "updatePlaylist").await;
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].method.as_str(), "POST");
}

#[tokio::test]
async fn a_server_with_extensions_but_not_song_lyrics_gets_the_classic_endpoint() {
    let server = MockServer::start().await;
    mount(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({ "openSubsonicExtensions": [
            { "name": "formPost", "versions": [1] },
            { "name": "transcodeOffset", "versions": [1] }
        ]})),
    )
    .await;
    mount(
        &server,
        "getLyrics",
        ok(json!({ "lyrics": { "value": "only words" } })),
    )
    .await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    let lyrics = client.lyrics("s", Some("A"), "T").await.unwrap().unwrap();
    assert!(!lyrics.timed);
    assert_eq!(lyrics.lines[0].text, "only words");
    assert_eq!(sent(&server, "getLyrics").await.len(), 1);
    assert!(sent(&server, "getLyricsBySongId").await.is_empty());
}

#[tokio::test]
async fn an_unsynced_entry_with_the_whole_text_in_one_value_is_split() {
    let (server, client) = start(true).await;
    let long_text: String = (0..MAX_LINES + 20)
        .map(|i| format!("line {i}\r\n"))
        .collect();
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": false, "line": [
                { "value": "\nverse one\r\nverse two\n\nverse three\n" },
                { "value": "after" }
            ]}
        ]}})),
    )
    .await;
    let lyrics = client.lyrics("s", None, "T").await.unwrap().unwrap();
    let texts: Vec<_> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        ["verse one", "verse two", "", "verse three", "", "after"]
    );

    // Still capped: lines and line length.
    let (server, client) = start(true).await;
    mount(
        &server,
        "getLyricsBySongId",
        ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": false, "line": [
                { "value": format!("{}\n{long_text}", "y".repeat(MAX_LINE_CHARS * 3)) }
            ]}
        ]}})),
    )
    .await;
    let lyrics = client.lyrics("s", None, "T").await.unwrap().unwrap();
    assert_eq!(lyrics.lines.len(), MAX_LINES);
    assert_eq!(lyrics.lines[0].text.chars().count(), MAX_LINE_CHARS);
}
