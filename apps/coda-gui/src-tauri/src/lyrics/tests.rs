//! `now_playing_lyrics`: local ids through the library (stale, unknown,
//! a sidecar `.lrc`) and Navidrome songs against a local `wiremock`
//! server (no real server is contacted).

use std::sync::Arc;
use std::time::Duration;

use coda_core::library::Library;
use coda_core::model::{Track, TrackMetadata};
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password};
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::cover::CoverService;

fn handle() -> BrowseHandle {
    BrowseHandle::new(Arc::new(CoverService::new(None)))
}

fn track_ref(source: SourceDto) -> TrackRef {
    TrackRef {
        source,
        title: Some("Song".into()),
        artist: Some("Band".into()),
        album: None,
        album_artist: None,
        track_no: None,
        disc_no: None,
        duration_ms: None,
        album_id: None,
        artist_id: None,
    }
}

fn local_ref(id: Option<&str>) -> TrackRef {
    track_ref(SourceDto::Local {
        id: id.map(Into::into),
    })
}

/// A library of one track, `song.flac` in `dir` (not real audio: the tag
/// read fails, so only a sidecar can give lyrics).
fn library_with_song(dir: &std::path::Path) -> BrowseHandle {
    std::fs::write(dir.join("song.flac"), b"not audio").unwrap();
    let browse = handle();
    let mut library = Library::new(dir.to_path_buf());
    library.extend([Track::local(
        dir.join("song.flac"),
        TrackMetadata {
            title: Some("Song".into()),
            ..TrackMetadata::default()
        },
    )]);
    browse.replace_library(Some(library));
    browse
}

#[tokio::test]
async fn a_local_sidecar_is_read_and_mapped() {
    let dir = tempfile::tempdir().unwrap();
    let browse = library_with_song(dir.path());
    std::fs::write(
        dir.path().join("song.lrc"),
        "[ti:Song]\n[00:01.50]first\n[00:03.00]\n[00:04.25]second\n",
    )
    .unwrap();
    let generation = browse.library_generation();
    let lyrics = lyrics_for(&browse, local_ref(Some(&format!("{generation}:0"))))
        .await
        .unwrap()
        .unwrap();
    assert!(lyrics.timed);
    let lines: Vec<_> = lyrics
        .lines
        .iter()
        .map(|l| (l.start_ms, l.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [
            (Some(1500), "first"),
            (Some(3000), ""),
            (Some(4250), "second")
        ]
    );
    // The DTO's JSON shape is what `types.ts` mirrors.
    let json = serde_json::to_value(&lyrics).unwrap();
    assert_eq!(
        json["lines"][0],
        json!({ "startMs": 1500, "text": "first" })
    );
}

#[tokio::test]
async fn a_local_track_without_lyrics_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let browse = library_with_song(dir.path());
    let generation = browse.library_generation();
    let id = format!("{generation}:0");
    assert_eq!(lyrics_for(&browse, local_ref(Some(&id))).await, Ok(None));

    // A blank sidecar is no lyrics either.
    std::fs::write(dir.path().join("song.lrc"), "\n\n").unwrap();
    assert_eq!(lyrics_for(&browse, local_ref(Some(&id))).await, Ok(None));
}

#[tokio::test]
async fn stale_unknown_and_missing_local_ids_have_none() {
    let dir = tempfile::tempdir().unwrap();
    let browse = library_with_song(dir.path());
    std::fs::write(dir.path().join("song.lrc"), "[00:01]words\n").unwrap();
    let generation = browse.library_generation();
    for id in [
        Some(format!("{}:0", generation + 1)),
        Some(format!("{generation}:7")),
        Some("garbage".into()),
        None,
    ] {
        assert_eq!(
            lyrics_for(&browse, local_ref(id.as_deref())).await,
            Ok(None),
            "{id:?}"
        );
    }
    // No library at all.
    assert_eq!(
        lyrics_for(&handle(), local_ref(Some("1:0"))).await,
        Ok(None)
    );
}

#[tokio::test]
async fn a_local_file_that_went_away_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let browse = library_with_song(dir.path());
    std::fs::remove_file(dir.path().join("song.flac")).unwrap();
    let generation = browse.library_generation();
    let error = lyrics_for(&browse, local_ref(Some(&format!("{generation}:0"))))
        .await
        .unwrap_err();
    assert!(!error.contains(dir.path().to_str().unwrap()), "{error}");
}

// ----- Navidrome -------------------------------------------------------------

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

async fn server() -> (MockServer, BrowseHandle) {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getOpenSubsonicExtensions"))
        .respond_with(ok(json!({ "openSubsonicExtensions": [
            { "name": "songLyrics", "versions": [1] }
        ]})))
        .mount(&server)
        .await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new("s3cret-pass".into()),
    )
    .unwrap();
    let browse = handle();
    browse.set_client(Some(client));
    (server, browse)
}

fn song(id: &str) -> TrackRef {
    track_ref(SourceDto::Navidrome {
        song_id: id.into(),
        cover_art: None,
    })
}

#[tokio::test]
async fn navidrome_lyrics_are_fetched_and_mapped() {
    let (server, browse) = server().await;
    Mock::given(path("/rest/getLyricsBySongId"))
        .respond_with(ok(json!({ "lyricsList": { "structuredLyrics": [
            { "synced": true, "line": [
                { "start": 2000, "value": "two" }, { "start": 500, "value": "one" }
            ]}
        ]}})))
        .mount(&server)
        .await;
    let lyrics = lyrics_for(&browse, song("s1")).await.unwrap().unwrap();
    assert!(lyrics.timed);
    assert_eq!(
        lyrics.lines,
        [
            LyricLineDto {
                start_ms: Some(500),
                text: "one".into()
            },
            LyricLineDto {
                start_ms: Some(2000),
                text: "two".into()
            },
        ]
    );
}

#[tokio::test]
async fn navidrome_without_lyrics_is_none_and_a_failure_is_an_error() {
    let (server, browse) = server().await;
    Mock::given(path("/rest/getLyricsBySongId"))
        .respond_with(ok(json!({ "lyricsList": {} })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(lyrics_for(&browse, song("s1")).await, Ok(None));

    Mock::given(path("/rest/getLyricsBySongId"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let error = lyrics_for(&browse, song("s1")).await.unwrap_err();
    assert!(!error.contains("s3cret-pass"), "{error}");

    // A bad id is never sent.
    assert_eq!(lyrics_for(&browse, song("")).await, Ok(None));
}

#[tokio::test]
async fn navidrome_without_a_server_is_an_error() {
    assert!(lyrics_for(&handle(), song("s1")).await.is_err());
}

#[test]
fn a_huge_start_does_not_overflow() {
    let dto = LyricsDto::from_lyrics(Lyrics {
        timed: true,
        lines: vec![lyrics::Line {
            start: Some(Duration::MAX),
            text: "x".into(),
        }],
    })
    .unwrap();
    assert_eq!(dto.lines[0].start_ms, Some(u64::MAX));
}
