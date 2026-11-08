//! Playlist edit tests: validation, messages, and edits against a local
//! `wiremock` server (no real server is contacted).

use std::sync::Arc;
use std::time::{Duration, Instant};

use coda_core::navidrome::client::parse_base_url;
use coda_core::navidrome::{Client, Password};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::commands::{create_edit, remove_edit, reorder_edit, submit};
use super::dto::EditStatus;
use super::*;
use crate::browse::BrowseHandle;
use crate::cover::CoverService;

const USER: &str = "listener";

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1", "type": "navidrome" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

/// A server without `formPost` (every request a GET, parameters in the
/// query) and a browse handle using it.
async fn server() -> (MockServer, BrowseHandle) {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getOpenSubsonicExtensions"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        USER.into(),
        Password::new("s3cret-pass".into()),
    )
    .unwrap();
    let browse = BrowseHandle::new(Arc::new(CoverService::new(None)));
    browse.set_client(Some(client));
    (server, browse)
}

/// `getPlaylist` answering `songs` (ids), with `song_count` stored.
async fn mount_playlist(server: &MockServer, songs: &[&str], song_count: usize) {
    let entries: Vec<Value> = songs
        .iter()
        .map(|id| json!({ "id": id, "title": id }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylist"))
        .respond_with(ok(json!({ "playlist": {
            "id": "pl1", "name": "Road", "owner": USER, "songCount": song_count, "entry": entries
        }})))
        .mount(server)
        .await;
}

async fn writes(server: &MockServer, endpoint: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == format!("/rest/{endpoint}"))
        .collect()
}

fn values(request: &Request, key: &str) -> Vec<String> {
    request
        .url
        .query_pairs()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .collect()
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

// ----- validation ---------------------------------------------------------

#[test]
fn names_are_trimmed_and_bounded() {
    assert_eq!(clean_name("  Road trip \n").unwrap(), "Road trip");
    assert_eq!(clean_name("a\tb").unwrap(), "a b");
    assert!(clean_name("   ").is_err());
    assert!(clean_name("\u{7}").is_err());
    assert!(clean_name(&"x".repeat(MAX_NAME_CHARS)).is_ok());
    assert!(clean_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
    assert_eq!(shown_name(""), "the playlist");
    assert_eq!(shown_name(" Mix\u{0} "), "Mix");
}

#[test]
fn ids_are_checked() {
    assert!(check_ids(&ids(&["s1", "s2"]), "song").is_ok());
    assert!(check_ids(&ids(&["s1", ""]), "song").is_err());
    assert!(check_ids(&["x".repeat(300)], "song").is_err());
    assert!(check_ids(&ids(&["a\nb"]), "song").is_err());
    let many = vec!["s".to_owned(); MAX_PLAYLIST_ENTRIES + 1];
    assert!(check_ids(&many, "song").is_err());
    assert!(check_playlist_id("").is_err());
    assert!(create_edit("", Vec::new()).is_err());
}

#[test]
fn removals_are_one_remove_or_a_replace_and_never_empty_the_playlist() {
    let expected = ids(&["a", "b", "c"]);
    match removal("pl".into(), expected.clone(), &[1], 0).unwrap() {
        EditOp::Remove { index, hidden, .. } => assert_eq!((index, hidden), (1, 0)),
        other => panic!("{other:?}"),
    }
    match removal("pl".into(), expected.clone(), &[2, 0, 2], 5).unwrap() {
        EditOp::Replace { songs, hidden, .. } => {
            assert_eq!(songs, ids(&["b"]));
            assert_eq!(hidden, 5);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        removal("pl".into(), expected.clone(), &[0, 1, 2], 0).unwrap_err(),
        LAST_SONG
    );
    assert_eq!(
        removal("pl".into(), expected.clone(), &[3], 0).unwrap_err(),
        STALE_PLAYLIST
    );
    assert!(removal("pl".into(), expected, &[], 0).is_err());
}

#[test]
fn reorders_must_be_permutations() {
    let expected = ids(&["a", "b", "c"]);
    assert_eq!(
        reorder("pl".into(), expected.clone(), &[0, 1, 2], 0).unwrap(),
        None
    );
    match reorder("pl".into(), expected.clone(), &[2, 0, 1], 0)
        .unwrap()
        .unwrap()
    {
        EditOp::Replace { songs, .. } => assert_eq!(songs, ids(&["c", "a", "b"])),
        other => panic!("{other:?}"),
    }
    for bad in [&[0, 1][..], &[0, 0, 1], &[0, 1, 3], &[0, 1, 2, 2]] {
        assert!(
            reorder("pl".into(), expected.clone(), bad, 0).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn hidden_confirmations_become_bounds() {
    assert_eq!(hidden_bound(HiddenDto::None), 0);
    assert_eq!(hidden_bound(HiddenDto::Some { count: 3 }), 3);
    assert_eq!(hidden_bound(HiddenDto::Suspect), usize::MAX);
}

#[test]
fn messages_follow_coda() {
    let mut edit = create_edit("Queue 2026-11-05", ids(&["s1", "s2"])).unwrap();
    edit.left_out = 3;
    let done = outcome(
        &edit,
        &EditResult::Done {
            created: Some("pl9".into()),
        },
    );
    assert_eq!(done.status, EditStatus::Done);
    assert_eq!(done.created.as_deref(), Some("pl9"));
    assert_eq!(
        done.message,
        "Created Queue 2026-11-05 with 2 songs (3 local tracks left out)"
    );
    let partial = outcome(
        &edit,
        &EditResult::Partial {
            created: Some("pl9".into()),
            added: 1,
            error: Error::Aborted,
        },
    );
    assert_eq!(partial.status, EditStatus::Partial);
    assert!(
        partial
            .message
            .starts_with("Created Queue 2026-11-05 with 1 of 2 songs; could not add the rest"),
        "{}",
        partial.message
    );
    let stale = outcome(&edit, &EditResult::Stale);
    assert_eq!(
        (stale.status, stale.message.as_str()),
        (EditStatus::Stale, STALE_PLAYLIST)
    );
    let failed = outcome(&edit, &EditResult::Failed(Error::Aborted));
    assert_eq!(failed.status, EditStatus::Failed);
    assert!(
        failed
            .message
            .starts_with("Could not create Queue 2026-11-05")
    );

    let remove = remove_edit(
        "pl".into(),
        "Road",
        ids(&["a", "b", "c"]),
        &[0, 2],
        HiddenDto::None,
    )
    .unwrap();
    assert_eq!(
        outcome(&remove, &EditResult::Done { created: None }).message,
        "Removed 2 songs from Road"
    );
    let order = reorder_edit(
        "pl".into(),
        "Road",
        ids(&["a", "b"]),
        &[1, 0],
        HiddenDto::None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        outcome(&order, &EditResult::Done { created: None }).message,
        "Saved the new order of Road"
    );
}

// ----- against a server -----------------------------------------------------

#[tokio::test]
async fn without_a_server_edits_are_refused() {
    let browse = BrowseHandle::new(Arc::new(CoverService::new(None)));
    let editor = PlaylistEditor::default();
    let outcome = submit(&editor, &browse, create_edit("Mix", Vec::new())).await;
    assert_eq!(outcome.status, EditStatus::Refused);
    assert_eq!(outcome.message, NO_SERVER);
    let outcome = submit(&editor, &browse, Err("bad".into())).await;
    assert_eq!(
        (outcome.status, outcome.message.as_str()),
        (EditStatus::Refused, "bad")
    );
}

#[tokio::test]
async fn create_sends_the_name_and_songs_in_order() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/createPlaylist"))
        .and(query_param("name", "Road trip"))
        .respond_with(ok(
            json!({ "playlist": { "id": "pl7", "name": "Road trip" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();
    let outcome = submit(
        &editor,
        &browse,
        create_edit(" Road trip ", ids(&["s2", "s1", "s2"])),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Done, "{}", outcome.message);
    assert_eq!(outcome.created.as_deref(), Some("pl7"));
    assert_eq!(outcome.message, "Created Road trip with 3 songs");
    let sent = writes(&server, "createPlaylist").await;
    assert_eq!(values(&sent[0], "songId"), ids(&["s2", "s1", "s2"]));
}

#[tokio::test]
async fn a_removal_rewrites_the_list_only_if_it_is_unchanged() {
    let (server, browse) = server().await;
    mount_playlist(&server, &["a", "b", "c"], 3).await;
    Mock::given(method("GET"))
        .and(path("/rest/createPlaylist"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();

    // Shown list differs from the server's: stale, nothing written.
    let outcome = submit(
        &editor,
        &browse,
        remove_edit(
            "pl1".into(),
            "Road",
            ids(&["a", "c"]),
            &[0],
            HiddenDto::None,
        ),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Stale);
    assert!(writes(&server, "createPlaylist").await.is_empty());

    // As shown: replaced without the entry.
    let outcome = submit(
        &editor,
        &browse,
        remove_edit(
            "pl1".into(),
            "Road",
            ids(&["a", "b", "c"]),
            &[1],
            HiddenDto::None,
        ),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Done, "{}", outcome.message);
    assert_eq!(outcome.message, "Removed 1 song from Road");
    let sent = writes(&server, "createPlaylist").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(values(&sent[0], "playlistId"), ids(&["pl1"]));
    assert_eq!(values(&sent[0], "songId"), ids(&["a", "c"]));
}

#[tokio::test]
async fn hidden_entries_need_the_confirmation_before_a_rewrite() {
    let (server, browse) = server().await;
    // Two entries the server stores but hides.
    mount_playlist(&server, &["a", "b"], 4).await;
    Mock::given(method("GET"))
        .and(path("/rest/createPlaylist"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();

    // Not confirmed: refused as stale by the refetch, nothing written.
    let outcome = submit(
        &editor,
        &browse,
        reorder_edit(
            "pl1".into(),
            "Road",
            ids(&["a", "b"]),
            &[1, 0],
            HiddenDto::None,
        )
        .map(Option::unwrap),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Stale);
    // Confirmed for fewer than are hidden now: still refused.
    let outcome = submit(
        &editor,
        &browse,
        reorder_edit(
            "pl1".into(),
            "Road",
            ids(&["a", "b"]),
            &[1, 0],
            HiddenDto::Some { count: 1 },
        )
        .map(Option::unwrap),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Stale);
    assert!(writes(&server, "createPlaylist").await.is_empty());

    // Confirmed: saved.
    let outcome = submit(
        &editor,
        &browse,
        reorder_edit(
            "pl1".into(),
            "Road",
            ids(&["a", "b"]),
            &[1, 0],
            HiddenDto::Some { count: 2 },
        )
        .map(Option::unwrap),
    )
    .await;
    assert_eq!(outcome.status, EditStatus::Done, "{}", outcome.message);
    let sent = writes(&server, "createPlaylist").await;
    assert_eq!(values(&sent[0], "songId"), ids(&["b", "a"]));
}

#[tokio::test]
async fn add_rename_and_delete_go_to_their_endpoints() {
    let (server, browse) = server().await;
    for endpoint in ["updatePlaylist", "deletePlaylist"] {
        Mock::given(method("GET"))
            .and(path(format!("/rest/{endpoint}")))
            .respond_with(ok(json!({})))
            .mount(&server)
            .await;
    }
    let editor = PlaylistEditor::default();
    let add = Edit {
        op: EditOp::Add {
            playlist_id: "pl1".into(),
            songs: ids(&["s1"]),
        },
        playlist: "Road".into(),
        old_name: String::new(),
        left_out: 0,
    };
    let outcome = submit(&editor, &browse, Ok(add)).await;
    assert_eq!(outcome.message, "Added 1 song to Road");
    let rename = Edit {
        op: EditOp::Rename {
            playlist_id: "pl1".into(),
            name: "Trip".into(),
        },
        playlist: "Trip".into(),
        old_name: "Road".into(),
        left_out: 0,
    };
    let outcome = submit(&editor, &browse, Ok(rename)).await;
    assert_eq!(outcome.message, "Renamed Road to Trip");
    let delete = Edit {
        op: EditOp::Delete {
            playlist_id: "pl1".into(),
        },
        playlist: "Trip".into(),
        old_name: String::new(),
        left_out: 0,
    };
    let outcome = submit(&editor, &browse, Ok(delete)).await;
    assert_eq!(outcome.status, EditStatus::Done);
    assert_eq!(outcome.message, "Deleted Trip");
    let updates = writes(&server, "updatePlaylist").await;
    assert_eq!(values(&updates[0], "songIdToAdd"), ids(&["s1"]));
    assert_eq!(values(&updates[1], "name"), ids(&["Trip"]));
    assert_eq!(writes(&server, "deletePlaylist").await.len(), 1);
}

#[tokio::test]
async fn a_failure_is_reported_with_the_servers_message() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/deletePlaylist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subsonic-response": {
                "status": "failed", "version": "1.16.1",
                "error": { "code": 50, "message": "not allowed" }
            }
        })))
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();
    let delete = Edit {
        op: EditOp::Delete {
            playlist_id: "pl1".into(),
        },
        playlist: "Road".into(),
        old_name: String::new(),
        left_out: 0,
    };
    let outcome = submit(&editor, &browse, Ok(delete)).await;
    assert_eq!(outcome.status, EditStatus::Failed);
    assert!(
        outcome.message.starts_with("Could not delete Road"),
        "{}",
        outcome.message
    );
    assert!(!outcome.message.contains("s3cret"));
}

#[tokio::test]
async fn edits_run_one_at_a_time() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/deletePlaylist"))
        .respond_with(ok(json!({})).set_delay(Duration::from_millis(200)))
        .mount(&server)
        .await;
    let editor = Arc::new(PlaylistEditor::default());
    let delete = |id: &str| Edit {
        op: EditOp::Delete {
            playlist_id: id.into(),
        },
        playlist: id.into(),
        old_name: String::new(),
        left_out: 0,
    };
    let started = Instant::now();
    let (a, b) = tokio::join!(
        submit(&editor, &browse, Ok(delete("pl1"))),
        submit(&editor, &browse, Ok(delete("pl2"))),
    );
    assert_eq!((a.status, b.status), (EditStatus::Done, EditStatus::Done));
    // In parallel both would take ~200 ms.
    assert!(
        started.elapsed() >= Duration::from_millis(390),
        "{:?}",
        started.elapsed()
    );
    // `finish` returns at once when idle.
    tokio::time::timeout(Duration::from_millis(100), editor.finish(EDIT_GRACE))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_client_change_during_an_edit_is_reported() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/deletePlaylist"))
        .respond_with(ok(json!({})).set_delay(Duration::from_millis(150)))
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();
    let delete = Edit {
        op: EditOp::Delete {
            playlist_id: "pl1".into(),
        },
        playlist: "Road".into(),
        old_name: String::new(),
        left_out: 0,
    };
    let swap = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        browse.set_client(None);
    };
    let queued = async {
        // Waits behind the first edit, which outlives the swap.
        tokio::time::sleep(Duration::from_millis(20)).await;
        submit(&editor, &browse, Ok(delete.clone())).await
    };
    let (outcome, second, ()) =
        tokio::join!(submit(&editor, &browse, Ok(delete.clone())), queued, swap);
    // The old server applied it: still `done`, with a note.
    assert_eq!(outcome.status, EditStatus::Done);
    assert_eq!(outcome.message, format!("Deleted Road ({CLIENT_CHANGED})"));
    // The queued edit takes the client current when it starts: none now.
    assert_eq!(
        (second.status, second.message.as_str()),
        (EditStatus::Refused, NO_SERVER)
    );
    assert_eq!(writes(&server, "deletePlaylist").await.len(), 1);
}

#[tokio::test]
async fn no_edit_starts_once_quitting_began() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/deletePlaylist"))
        .respond_with(ok(json!({})).set_delay(Duration::from_millis(150)))
        .mount(&server)
        .await;
    let editor = PlaylistEditor::default();
    let delete = |id: &str| Edit {
        op: EditOp::Delete {
            playlist_id: id.into(),
        },
        playlist: id.into(),
        old_name: String::new(),
        left_out: 0,
    };
    let queued = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        submit(&editor, &browse, Ok(delete("pl2"))).await
    };
    let quit = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        editor.finish(EDIT_GRACE).await;
    };
    let (running, second, ()) =
        tokio::join!(submit(&editor, &browse, Ok(delete("pl1"))), queued, quit);
    assert_eq!(running.status, EditStatus::Done);
    assert_eq!(
        (second.status, second.message.as_str()),
        (EditStatus::Refused, QUITTING)
    );
    assert_eq!(writes(&server, "deletePlaylist").await.len(), 1);
}

#[test]
fn removal_indices_are_bounded() {
    let expected = ids(&["a", "b"]);
    assert_eq!(
        removal("pl".into(), expected, &[0, 0, 0], 0).unwrap_err(),
        STALE_PLAYLIST
    );
}

#[tokio::test]
async fn playlists_say_whether_the_user_may_edit_them() {
    let (server, browse) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylists"))
        .respond_with(ok(json!({ "playlists": { "playlist": [
            { "id": "pl1", "name": "Mine", "owner": "Listener" },
            { "id": "pl2", "name": "Theirs", "owner": "other" },
            { "id": "pl3", "name": "Smart", "owner": USER, "readonly": true },
            { "id": "pl4", "name": "Unknown" }
        ]}})))
        .mount(&server)
        .await;
    mount_playlist(&server, &["a"], 1).await;
    let list = browse.navidrome_playlists().await.unwrap();
    let editable: Vec<(bool, Option<&str>)> = list
        .iter()
        .map(|p| (p.editable, p.read_only_reason.as_deref()))
        .collect();
    use coda_core::navidrome::browse::{READ_ONLY_FLAGGED, READ_ONLY_OTHER, READ_ONLY_UNKNOWN};
    assert_eq!(
        editable,
        [
            (true, None),
            (false, Some(READ_ONLY_OTHER)),
            (false, Some(READ_ONLY_FLAGGED)),
            (false, Some(READ_ONLY_UNKNOWN)),
        ]
    );
    let detail = browse.navidrome_playlist("pl1".into()).await.unwrap();
    assert!(detail.playlist.editable);
}
