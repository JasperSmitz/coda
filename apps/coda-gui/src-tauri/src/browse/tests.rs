//! Browse layer tests: local queries over a synthetic library, the id and
//! generation rule, and Navidrome queries against a local `wiremock`
//! server (no real server is contacted).

use std::path::PathBuf;
use std::time::Duration;

use coda_core::library::m3u::PlaylistFile;
use coda_core::model::TrackMetadata;
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::dto::*;
use super::*;

const PASSWORD: &str = "s3cret-pass";

fn handle() -> BrowseHandle {
    BrowseHandle::new(Arc::new(CoverService::new(None)))
}

fn local(rel: &str, artist: &str, album: &str, no: u32) -> Track {
    Track::local(
        PathBuf::from(format!("/m/{rel}")),
        TrackMetadata {
            title: Some(format!("T{no}")),
            artist: Some(artist.into()),
            album: Some(album.into()),
            track_no: Some(no),
            duration: Some(Duration::from_secs(60)),
            ..TrackMetadata::default()
        },
    )
}

/// A library with two artists, three albums, nested folders and a
/// playlist.
fn library_handle() -> BrowseHandle {
    let browse = handle();
    let mut library = Library::new("/m".into());
    library.extend([
        local("Band/Second/2.flac", "Band", "Second", 2),
        local("Band/Second/1.flac", "Band", "Second", 1),
        local("Band/First/1.flac", "Band", "First", 1),
        local("Solo/Only/1.flac", "Solo", "Only", 1),
        local("loose.flac", "Solo", "Only", 2),
    ]);
    library.add_playlist(PlaylistFile {
        path: "/m/mix.m3u".into(),
        name: "mix".into(),
        entries: vec![
            "/m/Solo/Only/1.flac".into(),
            "/m/gone.flac".into(),
            "/m/Solo/Only/1.flac".into(),
        ],
        unresolved: 1,
        cut: false,
    });
    browse.replace_library(Some(library));
    browse
}

#[test]
fn local_ids_carry_the_generation() {
    assert_eq!(local_id(3, 17), "3:17");
    assert_eq!(parse_local_id("3:17", 3), Ok(17));
    assert_eq!(
        parse_local_id("2:17", 3).unwrap_err().kind,
        BrowseErrorKind::Stale
    );
    for bad in ["", "3", "3:", ":1", "x:1", "3:-1", "3:1:2", "3:99999999999"] {
        assert_eq!(
            parse_local_id(bad, 3).unwrap_err().kind,
            BrowseErrorKind::NotFound,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn local_queries_need_a_library() {
    let browse = handle();
    let error = browse.local(local::artists).await.unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::NotConfigured);
}

#[tokio::test]
async fn local_artists_albums_and_tracks() {
    let browse = library_handle();
    let generation = browse.library_generation();
    let artists = browse.local(local::artists).await.unwrap();
    let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Band", "Solo"]);
    assert_eq!(artists[0].album_count, Some(2));
    assert_eq!(artists[0].track_count, Some(3));
    assert!(artists[0].id.starts_with(&format!("{generation}:")));

    let albums = browse
        .local(|v, c| local::albums(v, LocalAlbumOrder::Name, c))
        .await
        .unwrap();
    let names: Vec<&str> = albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["First", "Only", "Second"]);
    let second = &albums[2];
    assert_eq!(second.artist.as_deref(), Some("Band"));
    assert_eq!(second.artist_id.as_deref(), Some(artists[0].id.as_str()));
    assert_eq!(second.track_count, Some(2));
    assert_eq!(second.duration_ms, Some(120_000));
    assert!(second.cover_url.is_some());

    let id = artists[0].id.clone();
    let artist = browse
        .local(move |v, c| local::artist(v, &id, c))
        .await
        .unwrap();
    let names: Vec<&str> = artist.albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["First", "Second"]);

    // `s` on an artist: every album A–Z, each in album order.
    let id = artists[0].id.clone();
    let titles: Vec<(String, String)> = browse
        .local(move |v, _| {
            let ids = local::context_track_ids(&v, &LocalContextDto::Artist { id })?;
            Ok(ids
                .iter()
                .map(|t| {
                    let track = v.library.track(*t);
                    (track.album.clone().unwrap(), track.title.clone())
                })
                .collect())
        })
        .await
        .unwrap();
    let titles: Vec<(&str, &str)> = titles
        .iter()
        .map(|(a, t)| (a.as_str(), t.as_str()))
        .collect();
    assert_eq!(
        titles,
        [("First", "T1"), ("Second", "T1"), ("Second", "T2")]
    );

    let id = second.id.clone();
    let album = browse
        .local(move |v, c| local::album(v, &id, c))
        .await
        .unwrap();
    assert_eq!(album.album.name, "Second");
    let titles: Vec<&str> = album.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["T1", "T2"]);
    // Tracks link back to their album and artist and play by id.
    let track = &album.tracks[0];
    assert_eq!(track.album_id.as_deref(), Some(second.id.as_str()));
    assert_eq!(track.artist_id.as_deref(), Some(artists[0].id.as_str()));
    match &track.source {
        SourceDto::Local { id: Some(id) } => assert!(id.starts_with(&format!("{generation}:"))),
        other => panic!("{other:?}"),
    }
    // No path reaches the UI.
    let json = serde_json::to_string(&album).unwrap();
    assert!(!json.contains("/m/"), "{json}");
}

#[tokio::test]
async fn ids_from_an_older_generation_or_out_of_range_fail() {
    let browse = library_handle();
    let artists = browse.local(local::artists).await.unwrap();
    let old = artists[0].id.clone();
    let generation = browse.library_generation();
    let missing = local_id(generation, 99);
    let error = browse
        .local(move |v, c| local::album(v, &missing, c))
        .await
        .unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::NotFound);

    browse.replace_library(Some(Library::new("/m".into())));
    assert_eq!(browse.library_generation(), generation + 1);
    let error = browse
        .local(move |v, c| local::artist(v, &old, c))
        .await
        .unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::Stale);
}

#[tokio::test]
async fn local_folders_level_by_level() {
    let browse = library_handle();
    let root = browse
        .local(|v, c| local::folder(v, Vec::new(), c))
        .await
        .unwrap();
    assert_eq!(root.dirs, ["Band", "Solo"]);
    assert_eq!(root.tracks.len(), 1);
    assert_eq!(root.tracks[0].title, "T2");
    let band = browse
        .local(|v, c| local::folder(v, vec!["Band".into()], c))
        .await
        .unwrap();
    assert_eq!(band.dirs, ["First", "Second"]);
    assert!(band.tracks.is_empty());
    let all = browse
        .local(|v, _| {
            local::context_track_ids(
                &v,
                &LocalContextDto::Folder {
                    path: vec!["Band".into()],
                },
            )
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
    let error = browse
        .local(|v, _| {
            local::context_track_ids(
                &v,
                &LocalContextDto::Artist {
                    id: "nonsense".into(),
                },
            )
        })
        .await
        .unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::NotFound);
    let error = browse
        .local(|v, c| local::folder(v, vec!["Nope".into()], c))
        .await
        .unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::NotFound);
}

#[tokio::test]
async fn local_playlists_keep_duplicates_and_warn_about_missing_entries() {
    let browse = library_handle();
    let playlists = browse.local(local::playlists).await.unwrap();
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0].name, "mix");
    assert_eq!(playlists[0].track_count, Some(2));
    let id = playlists[0].id.clone();
    let detail = browse
        .local(move |v, c| local::playlist(v, &id, c))
        .await
        .unwrap();
    assert_eq!(detail.tracks.len(), 2);
    assert_eq!(detail.missing, 2);
    assert_eq!(
        detail.warning.as_deref(),
        Some("2 entries not found in the library")
    );
}

#[tokio::test]
async fn local_search_groups_artists_albums_and_tracks() {
    let browse = library_handle();
    let found = browse
        .local(|v, c| local::search(v, "sec", c))
        .await
        .unwrap();
    assert!(found.artists.is_empty());
    assert_eq!(found.albums.len(), 1);
    assert_eq!(found.albums[0].name, "Second");
    assert_eq!(found.tracks.len(), 2);
    assert_eq!(found.total_tracks, 2);
    let found = browse
        .local(|v, c| local::search(v, "solo", c))
        .await
        .unwrap();
    assert_eq!(found.artists.len(), 1);
    let blank = browse
        .local(|v, c| local::search(v, "  ", c))
        .await
        .unwrap();
    assert!(blank.tracks.is_empty() && blank.artists.is_empty());
}

// ----- Navidrome ----------------------------------------------------------

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

async fn server() -> (MockServer, BrowseHandle) {
    let server = MockServer::start().await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    let browse = handle();
    browse.set_client(Some(client));
    (server, browse)
}

async fn mount(server: &MockServer, endpoint: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/rest/{endpoint}")))
        .respond_with(ok(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn navidrome_needs_a_server() {
    let browse = handle();
    let error = browse.navidrome_artists().await.unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::NotConfigured);
}

#[tokio::test]
async fn navidrome_artists_albums_and_tracks() {
    let (server, browse) = server().await;
    mount(
        &server,
        "getArtists",
        json!({ "artists": { "index": [
            { "name": "A", "artist": [{ "id": "ar1", "name": "ABBA", "albumCount": 2, "coverArt": "ar-1" }] }
        ]}}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar1"))
        .respond_with(ok(json!({ "artist": { "id": "ar1", "album": [
            { "id": "al1", "name": "Arrival", "artist": "ABBA", "artistId": "ar1", "year": 1976, "songCount": 2, "coverArt": "al-1" },
            { "id": "al2", "name": "Voulez-Vous" }
        ]}})))
        .mount(&server)
        .await;
    for (id, songs) in [
        (
            "al1",
            json!([{ "id": "s1", "title": "Dancing Queen", "duration": 230, "artistId": "ar-guest" }, { "id": "s2", "title": "Money" }]),
        ),
        ("al2", json!([{ "id": "s3", "title": "Chiquitita" }])),
    ] {
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum"))
            .and(query_param("id", id))
            .respond_with(ok(json!({ "album": {
                "id": id, "name": id, "artist": "ABBA", "year": 1976,
                "coverArt": format!("cover-{id}"), "artistId": "ar1", "song": songs
            }})))
            .mount(&server)
            .await;
    }

    let artists = browse.navidrome_artists().await.unwrap();
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].name, "ABBA");
    assert_eq!(artists[0].album_count, Some(2));
    assert!(artists[0].cover_url.is_some());

    let albums = browse.navidrome_artist("ar1".into()).await.unwrap();
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].year, Some(1976));
    assert_eq!(albums[0].artist_id.as_deref(), Some("ar1"));
    assert!(albums[0].cover_url.is_some());
    assert!(albums[1].cover_url.is_none());

    let album = browse.navidrome_album("al1".into()).await.unwrap();
    assert_eq!(album.tracks.len(), 2);
    assert_eq!(album.tracks[0].album_id.as_deref(), Some("al1"));
    assert_eq!(album.album.track_count, Some(2));
    // The header is the album's own: the album artist's id (not the first
    // song's track artist), the year and the name.
    assert_eq!(album.album.artist_id.as_deref(), Some("ar1"));
    assert_eq!(album.album.artist.as_deref(), Some("ABBA"));
    assert_eq!(album.album.year, Some(1976));
    assert_eq!(album.album.name, "al1");
    assert_eq!(album.tracks[0].artist_id.as_deref(), Some("ar-guest"));
    // One duration unknown: no total.
    assert_eq!(album.album.duration_ms, None);
    assert!(album.album.cover_url.is_some());

    // `s` on an artist: every album's tracks, albums in served order.
    let tracks = browse.navidrome_artist_tracks("ar1".into()).await.unwrap();
    let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Dancing Queen", "Money", "Chiquitita"]);

    // Nothing secret reaches the UI.
    let json = serde_json::to_string(&tracks).unwrap();
    assert!(!json.contains(PASSWORD) && !json.contains("token") && !json.contains("127.0.0.1"));
}

#[tokio::test]
async fn navidrome_album_pages_continue_until_a_short_page() {
    let (server, browse) = server().await;
    let full: Vec<Value> = (0..navidrome::ALBUM_PAGE)
        .map(|i| json!({ "id": format!("al{i}"), "name": format!("Album {i}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2"))
        .and(query_param("type", "newest"))
        .and(query_param("offset", "0"))
        .respond_with(ok(json!({ "albumList2": { "album": full } })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2"))
        .and(query_param("type", "newest"))
        .and(query_param("offset", navidrome::ALBUM_PAGE.to_string()))
        .respond_with(ok(
            json!({ "albumList2": { "album": [{ "id": "last", "name": "Last" }] } }),
        ))
        .mount(&server)
        .await;
    let page = browse
        .navidrome_albums(AlbumSortDto::Newest, 0)
        .await
        .unwrap();
    assert_eq!(page.albums.len(), navidrome::ALBUM_PAGE);
    assert_eq!(page.next_offset, Some(navidrome::ALBUM_PAGE));
    let page = browse
        .navidrome_albums(AlbumSortDto::Newest, navidrome::ALBUM_PAGE)
        .await
        .unwrap();
    assert_eq!(page.albums.len(), 1);
    assert_eq!(page.next_offset, None);
}

#[tokio::test]
async fn navidrome_playlists_carry_the_hidden_entries_warning() {
    let (server, browse) = server().await;
    mount(
        &server,
        "getPlaylists",
        json!({ "playlists": { "playlist": [
            { "id": "pl1", "name": "Road", "songCount": 4, "owner": "listener", "coverArt": "pl-1" }
        ]}}),
    )
    .await;
    mount(
        &server,
        "getPlaylist",
        json!({ "playlist": { "id": "pl1", "name": "Road", "songCount": 4, "entry": [
            { "id": "s1", "title": "One" },
            { "id": "s1", "title": "One" }
        ]}}),
    )
    .await;
    let playlists = browse.navidrome_playlists().await.unwrap();
    assert_eq!(playlists[0].name, "Road");
    assert_eq!(playlists[0].owner.as_deref(), Some("listener"));
    assert!(playlists[0].cover_url.is_some());
    let detail = browse.navidrome_playlist("pl1".into()).await.unwrap();
    assert_eq!(detail.tracks.len(), 2, "duplicates kept");
    assert_eq!(detail.hidden, HiddenDto::Some { count: 2 });
    assert_eq!(
        detail.warning.as_deref(),
        Some("Navidrome hides 2 entries (missing files or other libraries)")
    );
}

/// The library of [`library_handle`] with lyrics on three tracks.
fn lyrics_handle() -> BrowseHandle {
    use coda_core::lyrics::{index::IndexedLyrics, parse};
    let browse = library_handle();
    let lyrics = |text: &str| IndexedLyrics::new(&parse(text)).unwrap();
    let mut writer = browse.try_write_local().unwrap();
    writer.library_mut().unwrap().add_lyrics([
        ("/m/loose.flac".into(), lyrics("Under the Sun\nwe walk")),
        (
            "/m/Band/First/1.flac".into(),
            lyrics("sun shines\non the bay"),
        ),
        ("/m/Solo/Only/1.flac".into(), lyrics("rain all day")),
    ]);
    drop(writer);
    browse
}

#[tokio::test]
async fn local_lyrics_search_excludes_ids_and_splits_snippets() {
    let browse = lyrics_handle();
    let found = browse
        .local(|v, c| local::lyrics_search(v, "SUN", &[], c))
        .await
        .unwrap();
    assert_eq!(found.total, 2);
    // Browse order: folders first, then the loose track.
    let titles: Vec<_> = found
        .matches
        .iter()
        .map(|m| m.track.title.as_str())
        .collect();
    assert_eq!(titles, ["T1", "T2"]);
    let parts: Vec<_> = found.matches[1]
        .snippet
        .iter()
        .map(|p| (p.text.as_str(), p.matched))
        .collect();
    assert_eq!(parts, [("Under the ", false), ("Sun", true)]);
    // A phrase across two lines.
    let found = browse
        .local(|v, c| local::lyrics_search(v, "shines on", &[], c))
        .await
        .unwrap();
    let parts: Vec<_> = found.matches[0]
        .snippet
        .iter()
        .map(|p| (p.text.as_str(), p.matched))
        .collect();
    assert_eq!(
        parts,
        [
            ("sun ", false),
            ("shines", true),
            (" / ", false),
            ("on", true),
            (" the bay", false)
        ]
    );

    // The main search's songs are left out, and not counted.
    let generation = browse.library_generation();
    let first = found.matches[0].track.clone();
    let SourceDto::Local { id: Some(id) } = first.source else {
        panic!("local id");
    };
    let exclude = vec![id, "junk".into(), local_id(generation, 999)];
    let found = browse
        .local(move |v, c| local::lyrics_search(v, "sun", &exclude, c))
        .await
        .unwrap();
    assert_eq!(found.total, 1);
    assert_eq!(found.matches[0].track.title, "T2");
    // Ids from an older library make the search stale.
    let stale = vec![local_id(generation + 7, 0)];
    let error = browse
        .local(move |v, c| local::lyrics_search(v, "sun", &stale, c))
        .await
        .unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::Stale);
    // Short queries find nothing.
    let found = browse
        .local(|v, c| local::lyrics_search(v, "su", &[], c))
        .await
        .unwrap();
    assert_eq!((found.total, found.matches.len()), (0, 0));
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn snippet_parts_follow_the_highlights() {
    use coda_core::lyrics::index::Snippet;
    let parts = |text: &str, highlights: Vec<std::ops::Range<usize>>| {
        local::snippet_parts(&Snippet {
            text: text.into(),
            highlights,
        })
        .into_iter()
        .map(|p| (p.text, p.matched))
        .collect::<Vec<_>>()
    };
    assert_eq!(
        parts("été x", vec![0..3]),
        [("été".into(), true), (" x".into(), false)]
    );
    assert_eq!(parts("abc", vec![]), [("abc".into(), false)]);
    // Out-of-range or overlapping ranges never panic.
    assert_eq!(
        parts("abc", vec![1..9, 0..1]),
        [("a".into(), false), ("bc".into(), true)]
    );
}

#[tokio::test]
async fn navidrome_search_and_random_songs() {
    let (server, browse) = server().await;
    mount(
        &server,
        "search3",
        json!({ "searchResult3": {
            "artist": [{ "id": "ar1", "name": "ABBA" }],
            "album": [{ "id": "al1", "name": "Arrival" }],
            "song": [{ "id": "s1", "title": "Dancing Queen", "albumId": "al1", "artistId": "ar1" }]
        }}),
    )
    .await;
    mount(
        &server,
        "getRandomSongs",
        json!({ "randomSongs": { "song": [{ "id": "s9", "title": "Lucky" }] } }),
    )
    .await;
    let found = browse.navidrome_search(" abba ".into()).await.unwrap();
    assert_eq!(found.artists.len(), 1);
    assert_eq!(found.albums.len(), 1);
    assert_eq!(found.tracks[0].album_id.as_deref(), Some("al1"));
    assert_eq!(found.tracks[0].artist_id.as_deref(), Some("ar1"));
    let blank = browse.navidrome_search("   ".into()).await.unwrap();
    assert!(blank.tracks.is_empty());
    let random = browse.navidrome_random_songs().await.unwrap();
    assert_eq!(random[0].title, "Lucky");
}

#[tokio::test]
async fn navidrome_failures_and_stale_results_are_errors() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtists"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let error = browse.navidrome_artists().await.unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::Failed);
    assert!(!error.message.contains(PASSWORD), "{}", error.message);

    // A result that arrives after the client changed is dropped.
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylists"))
        .respond_with(
            ok(json!({ "playlists": { "playlist": [] } })).set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let pending = {
        let browse = browse.clone();
        tokio::spawn(async move { browse.navidrome_playlists().await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, other) = self::server().await;
    let (client, _) = other.client().unwrap();
    browse.set_client(Some(client));
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error.kind, BrowseErrorKind::Stale);
}

#[tokio::test]
async fn navidrome_requests_are_limited_to_the_fetch_cap() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylists"))
        .respond_with(
            ok(json!({ "playlists": { "playlist": [] } })).set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let tasks: Vec<_> = (0..MAX_CONCURRENT_FETCHES * 2)
        .map(|_| {
            let browse = browse.clone();
            tokio::spawn(async move { browse.navidrome_playlists().await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // While the first wave waits on the server, the rest wait for permits.
    assert_eq!(browse.0.fetches.available_permits(), 0);
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(browse.0.fetches.available_permits(), MAX_CONCURRENT_FETCHES);
}

#[tokio::test]
async fn ids_and_queries_from_the_ui_are_bounded() {
    let (_server, browse) = server().await;
    let long = "x".repeat(MAX_INPUT_CHARS + 1);
    for result in [
        browse.navidrome_album(long.clone()).await.map(drop),
        browse.navidrome_artist(String::new()).await.map(drop),
        browse.navidrome_playlist(long.clone()).await.map(drop),
        browse.navidrome_artist_tracks(long.clone()).await.map(drop),
    ] {
        assert_eq!(result.unwrap_err().kind, BrowseErrorKind::NotFound);
    }
    assert_eq!(
        cap_query(&format!("  {long}  ")).chars().count(),
        MAX_INPUT_CHARS
    );
    assert_eq!(cap_query("  abba "), "abba");
}
