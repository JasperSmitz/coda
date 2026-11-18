//! Mixes command tests: the store through the shared file (a temporary
//! one), errors, and the editor's searches over a synthetic library and a
//! local `wiremock` server (no real server is contacted).

use std::sync::Arc;
use std::time::Duration;

use coda_core::library::Library;
use coda_core::model::{Track, TrackMetadata};
use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::cover::CoverService;

fn handle() -> BrowseHandle {
    BrowseHandle::new(Arc::new(CoverService::new(None)))
}

fn track(rel: &str, artist: &str, album: &str, genre: &str) -> Track {
    Track::local(
        PathBuf::from(format!("/m/{rel}")),
        TrackMetadata {
            title: Some(rel.into()),
            artist: Some(artist.into()),
            album: Some(album.into()),
            duration: Some(Duration::from_secs(60)),
            genres: vec![genre.into()],
            ..TrackMetadata::default()
        },
    )
}

fn library_handle() -> BrowseHandle {
    let browse = handle();
    let mut library = Library::new("/m".into());
    library.extend([
        track("y/f/1.flac", "Yes", "Fragile", "Prog Rock"),
        track("y/f/2.flac", "Yes", "Fragile", "Prog Rock"),
        track("k/r/1.flac", "King Crimson", "Red", "Prog Rock"),
        track("m/k/1.flac", "Miles Davis", "Kind of Blue", "Jazz"),
    ]);
    browse.replace_library(Some(library));
    browse
}

fn input(name: &str, source: SourceKind, ingredients: Vec<IngredientDto>) -> MixInputDto {
    MixInputDto {
        name: name.into(),
        source,
        ingredients,
    }
}

fn genre(name: &str) -> IngredientDto {
    IngredientDto::Genre { name: name.into() }
}

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

fn client(server: &MockServer, user: &str) -> Client {
    Client::new(
        parse_base_url(&server.uri()).unwrap(),
        user.into(),
        Password::new("s3cret-pass".into()),
    )
    .unwrap()
}

#[tokio::test]
async fn mixes_are_added_listed_updated_and_deleted_in_the_shared_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = MixesFile(Some(dir.path().join("coda/mixes.json")));
    let browse = handle();
    // No file yet: an empty list.
    assert!(
        list(&file, &browse, SourceKind::Local)
            .await
            .unwrap()
            .is_empty()
    );
    let added = add(
        &file,
        &browse,
        input(
            "  Prog  ",
            SourceKind::Local,
            vec![
                genre("Prog Rock"),
                IngredientDto::Album {
                    artist: "Yes".into(),
                    name: "Fragile".into(),
                    id: Some("ignored for local".into()),
                },
            ],
        ),
    )
    .await
    .unwrap();
    // Cleaned as saved: the name trimmed, a local album's id dropped.
    assert_eq!(added.name, "Prog");
    assert_eq!(added.source, SourceKind::Local);
    assert_eq!(
        added.ingredients[1],
        IngredientDto::Album {
            artist: "Yes".into(),
            name: "Fragile".into(),
            id: None
        }
    );
    let listed = list(&file, &browse, SourceKind::Local).await.unwrap();
    assert_eq!(listed, vec![added.clone()]);

    let updated = update(
        &file,
        &browse,
        added.id.clone(),
        input("Jazz", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap();
    assert_eq!(updated.id, added.id);
    assert_eq!(updated.ingredients, vec![genre("Jazz")]);
    // The file is what both apps read: the change is in it.
    let (saved, _) = store::load(file.0.as_ref().unwrap());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "Jazz");

    delete(&file, &browse, added.id.clone(), SourceKind::Local)
        .await
        .unwrap();
    assert!(
        list(&file, &browse, SourceKind::Local)
            .await
            .unwrap()
            .is_empty()
    );
    // Deleting one already gone is no error.
    delete(&file, &browse, added.id, SourceKind::Local)
        .await
        .unwrap();
}

#[tokio::test]
async fn mixes_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let file = MixesFile(Some(dir.path().join("mixes.json")));
    let browse = handle();
    let err = add(
        &file,
        &browse,
        input(" ", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::Invalid);
    assert_eq!(err.message, "A mix needs a name");
    let err = add(&file, &browse, input("x", SourceKind::Local, vec![]))
        .await
        .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::Invalid);
    let err = update(
        &file,
        &browse,
        "0123456789abcdef".into(),
        input("x", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::NotFound);
    assert_eq!(
        delete(&file, &browse, "../etc".into(), SourceKind::Local)
            .await
            .unwrap_err()
            .kind,
        MixErrorKind::NotFound
    );
    // A Navidrome mix needs a server.
    let err = add(
        &file,
        &browse,
        input("x", SourceKind::Navidrome, vec![genre("Rock")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::NotConfigured);
    // No data folder: nothing to list, nothing saved.
    let none = MixesFile(None);
    assert!(
        list(&none, &browse, SourceKind::Local)
            .await
            .unwrap()
            .is_empty()
    );
    let err = add(
        &none,
        &browse,
        input("x", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::NotConfigured);
    // A file of a newer version is left alone and said so.
    std::fs::write(file.0.as_ref().unwrap(), r#"{"version": 99, "mixes": []}"#).unwrap();
    assert_eq!(
        list(&file, &browse, SourceKind::Local)
            .await
            .unwrap_err()
            .kind,
        MixErrorKind::Newer
    );
    let err = add(
        &file,
        &browse,
        input("x", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::Newer);
}

#[tokio::test]
async fn add_to_mix_appends_to_the_saved_recipe() {
    let dir = tempfile::tempdir().unwrap();
    let file = MixesFile(Some(dir.path().join("mixes.json")));
    let browse = handle();
    let saved = add(
        &file,
        &browse,
        input("Prog", SourceKind::Local, vec![genre("Prog Rock")]),
    )
    .await
    .unwrap();
    // Changed elsewhere after the picker listed it: both changes kept.
    update(
        &file,
        &browse,
        saved.id.clone(),
        input(
            "Prog 2",
            SourceKind::Local,
            vec![genre("Prog Rock"), genre("Jazz")],
        ),
    )
    .await
    .unwrap();
    let added = add_ingredient(
        &file,
        &browse,
        saved.id.clone(),
        SourceKind::Local,
        genre("Rock"),
    )
    .await
    .unwrap();
    assert_eq!(added.name, "Prog 2");
    assert_eq!(
        added.ingredients,
        vec![genre("Prog Rock"), genre("Jazz"), genre("Rock")]
    );
    let kind = |result: MixResult<MixDto>| result.unwrap_err().kind;
    assert_eq!(
        kind(
            add_ingredient(
                &file,
                &browse,
                saved.id.clone(),
                SourceKind::Local,
                genre("jazz")
            )
            .await
        ),
        MixErrorKind::AlreadyIn
    );
    assert_eq!(
        kind(
            add_ingredient(
                &file,
                &browse,
                "0123456789abcdef".into(),
                SourceKind::Local,
                genre("x")
            )
            .await
        ),
        MixErrorKind::NotFound
    );
    // Full now (50 genres).
    let many: Vec<IngredientDto> = (0..coda_core::mix::MAX_INGREDIENTS)
        .map(|i| genre(&format!("g{i}")))
        .collect();
    update(
        &file,
        &browse,
        saved.id.clone(),
        input("Prog", SourceKind::Local, many),
    )
    .await
    .unwrap();
    let err = add_ingredient(&file, &browse, saved.id, SourceKind::Local, genre("x"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::MixFull);
    assert_eq!(
        serde_json::to_value(&err).unwrap()["kind"],
        json!("mixFull")
    );
}

#[tokio::test]
async fn navidrome_mixes_belong_to_the_current_server() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let file = MixesFile(Some(dir.path().join("mixes.json")));
    let browse = handle();
    browse.set_client(Some(client(&server, "listener")));
    let rock = add(
        &file,
        &browse,
        input(
            "Rock",
            SourceKind::Navidrome,
            vec![IngredientDto::Artist {
                name: "Yes".into(),
                id: Some("ar1".into()),
            }],
        ),
    )
    .await
    .unwrap();
    assert_eq!(rock.source, SourceKind::Navidrome);
    add(
        &file,
        &browse,
        input("Local", SourceKind::Local, vec![genre("Jazz")]),
    )
    .await
    .unwrap();
    let listed = list(&file, &browse, SourceKind::Navidrome).await.unwrap();
    assert_eq!(listed, vec![rock.clone()]);
    // No URL, user or password in the shared file.
    let text = std::fs::read_to_string(file.0.as_ref().unwrap()).unwrap();
    assert!(!text.contains("listener") && !text.contains("127.0.0.1") && !text.contains("s3cret"));
    // Another account on the same server: not its mixes.
    browse.set_client(Some(client(&server, "other")));
    assert!(
        list(&file, &browse, SourceKind::Navidrome)
            .await
            .unwrap()
            .is_empty()
    );
    // An editor opened under the first account and saved (or deleted)
    // after the switch: refused, and the first account's mix is unchanged
    // (not moved to this account, not deleted).
    let err = update(
        &file,
        &browse,
        rock.id.clone(),
        input("Moved", SourceKind::Navidrome, vec![genre("Rock")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::OtherSource);
    // Nor turned into a local mix.
    let err = update(
        &file,
        &browse,
        rock.id.clone(),
        input("Moved", SourceKind::Local, vec![genre("Rock")]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::OtherSource);
    let err = delete(&file, &browse, rock.id.clone(), SourceKind::Navidrome)
        .await
        .unwrap_err();
    assert_eq!(err.kind, MixErrorKind::OtherSource);
    browse.set_client(Some(client(&server, "listener")));
    assert_eq!(
        list(&file, &browse, SourceKind::Navidrome).await.unwrap(),
        vec![rock.clone()]
    );
    // Back on its own account it can be deleted.
    delete(&file, &browse, rock.id.clone(), SourceKind::Navidrome)
        .await
        .unwrap();
    assert!(
        list(&file, &browse, SourceKind::Navidrome)
            .await
            .unwrap()
            .is_empty()
    );
    browse.set_client(None);
    assert_eq!(
        list(&file, &browse, SourceKind::Navidrome)
            .await
            .unwrap_err()
            .kind,
        MixErrorKind::NotConfigured
    );
}

#[tokio::test]
async fn local_searches_find_ingredients_by_name() {
    let browse = library_handle();
    let genres = search_genres(&browse, SourceKind::Local, "prog".into())
        .await
        .unwrap();
    assert_eq!(genres, vec![genre("Prog Rock")]);
    // An empty query lists every genre.
    let all = search_genres(&browse, SourceKind::Local, " ".into())
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let artists = search_artists(&browse, SourceKind::Local, "king".into())
        .await
        .unwrap();
    assert_eq!(
        artists,
        vec![IngredientDto::Artist {
            name: "King Crimson".into(),
            id: None
        }]
    );
    let albums = search_albums(&browse, SourceKind::Local, "frag".into())
        .await
        .unwrap();
    assert_eq!(
        albums,
        vec![IngredientDto::Album {
            artist: "Yes".into(),
            name: "Fragile".into(),
            id: None
        }]
    );
    assert!(
        search_artists(&browse, SourceKind::Local, "".into())
            .await
            .unwrap()
            .is_empty()
    );
    // What the search offers resolves: the found names match the library.
    let found = browse.read_library(|view| {
        let library = view.unwrap().library;
        albums
            .into_iter()
            .chain(artists)
            .chain(genres)
            .map(|i| coda_core::mix::local::pool(library, &Ingredient::from(i)).len())
            .collect::<Vec<_>>()
    });
    assert_eq!(found, vec![2, 1, 3]);
    // No library: the browse error.
    let err = search_genres(&handle(), SourceKind::Local, "x".into())
        .await
        .unwrap_err();
    assert_eq!(err.kind, BrowseErrorKind::NotConfigured);
}

#[tokio::test]
async fn navidrome_searches_give_exact_genres_ids_and_album_artists() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getGenres"))
        .respond_with(ok(json!({ "genres": { "genre": [
            { "value": "Prog Rock ", "songCount": 12, "albumCount": 3 },
            { "value": "Jazz", "songCount": 2, "albumCount": 1 }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "yes"))
        .respond_with(ok(json!({ "searchResult3": {
            "artist": [{ "id": "ar1", "name": "Yes" }],
            "album": [
                { "id": "al1", "name": "Fragile", "artist": "Yes" },
                { "id": "al2", "name": "No artist" }
            ]
        }})))
        .mount(&server)
        .await;
    let browse = handle();
    browse.set_client(Some(client(&server, "listener")));
    let genres = search_genres(&browse, SourceKind::Navidrome, "prog".into())
        .await
        .unwrap();
    // The server's exact value (trailing space kept), as random songs ask.
    assert_eq!(genres, vec![genre("Prog Rock ")]);
    let artists = search_artists(&browse, SourceKind::Navidrome, "yes".into())
        .await
        .unwrap();
    assert_eq!(
        artists,
        vec![IngredientDto::Artist {
            name: "Yes".into(),
            id: Some("ar1".into())
        }]
    );
    let albums = search_albums(&browse, SourceKind::Navidrome, "yes".into())
        .await
        .unwrap();
    assert_eq!(
        albums,
        vec![IngredientDto::Album {
            artist: "Yes".into(),
            name: "Fragile".into(),
            id: Some("al1".into())
        }]
    );
    browse.set_client(None);
    let err = search_genres(&browse, SourceKind::Navidrome, "x".into())
        .await
        .unwrap_err();
    assert_eq!(err.kind, BrowseErrorKind::NotConfigured);
}

#[test]
fn ingredients_round_trip_the_ui_json() {
    let value = json!([
        {"kind": "genre", "name": "Jazz"},
        {"kind": "artist", "name": "Yes", "id": "ar1"},
        {"kind": "album", "artist": "Yes", "name": "Fragile"}
    ]);
    let parsed: Vec<IngredientDto> = serde_json::from_value(value).unwrap();
    assert_eq!(
        parsed[2],
        IngredientDto::Album {
            artist: "Yes".into(),
            name: "Fragile".into(),
            id: None
        }
    );
    let back = serde_json::to_value(&parsed[1]).unwrap();
    assert_eq!(back, json!({"kind": "artist", "name": "Yes", "id": "ar1"}));
    let err = MixErrorDto::from(StoreError::Full);
    assert_eq!(
        serde_json::to_value(&err).unwrap(),
        json!({"kind": "full", "message": "There are already 500 mixes"})
    );
}
