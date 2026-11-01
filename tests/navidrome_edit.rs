//! Navidrome playlist writes and edit jobs against a local `wiremock`
//! server. No test contacts a real server.

use coda_player::navidrome::browse::Generation;
use coda_player::navidrome::client::{
    MAX_GET_URL_BYTES, MAX_POST_BODY_BYTES, Transport, parse_base_url,
};
use coda_player::navidrome::edit::{self, EditId, EditOp, EditRequest, EditResult};
use coda_player::navidrome::{Client, Error, Hidden, Password};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

/// A server whose `getOpenSubsonicExtensions` lists `formPost` (as
/// Navidrome's does) or not.
async fn start(form_post: bool) -> (MockServer, Client) {
    let server = MockServer::start().await;
    let extensions = if form_post {
        ok(json!({ "openSubsonicExtensions": [
            { "name": "transcodeOffset", "versions": [1] },
            { "name": "formPost", "versions": [1] },
            { "name": "songLyrics", "versions": [1, 2] }
        ]}))
    } else {
        // A plain Subsonic server: no such endpoint.
        ResponseTemplate::new(404)
    };
    mount_any(&server, "getOpenSubsonicExtensions", extensions).await;
    let client = client_at(&server.uri());
    (server, client)
}

/// Answers `endpoint` on any method.
async fn mount_any(server: &MockServer, endpoint: &str, response: ResponseTemplate) {
    Mock::given(path(format!("/rest/{endpoint}")))
        .respond_with(response)
        .mount(server)
        .await;
}

/// The parameters a request carried: the query for GET, the form body for
/// POST.
fn params(request: &Request) -> Vec<(String, String)> {
    let raw = if request.method.as_str() == "POST" {
        String::from_utf8(request.body.clone()).unwrap()
    } else {
        request.url.query().unwrap_or("").to_owned()
    };
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn values(params: &[(String, String)], key: &str) -> Vec<String> {
    params
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .collect()
}

fn one(params: &[(String, String)], key: &str) -> Option<String> {
    match values(params, key).as_slice() {
        [] => None,
        [v] => Some(v.clone()),
        many => panic!("{key} sent {} times", many.len()),
    }
}

/// Requests to `endpoint`, in order.
async fn sent(server: &MockServer, endpoint: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == format!("/rest/{endpoint}"))
        .collect()
}

fn ids(n: usize, len: usize) -> Vec<String> {
    (0..n).map(|i| format!("{i:0len$}")).collect()
}

fn playlist_body(id: &str, songs: &[String]) -> Value {
    json!({ "playlist": {
        "id": id, "name": "x", "owner": USER,
        "entry": songs.iter().map(|s| json!({ "id": s, "title": s })).collect::<Vec<_>>()
    }})
}

fn assert_signed(params: &[(String, String)]) {
    assert_eq!(one(params, "u").as_deref(), Some(USER));
    assert_eq!(one(params, "v").as_deref(), Some("1.16.1"));
    assert_eq!(one(params, "c").as_deref(), Some("coda"));
    assert_eq!(one(params, "f").as_deref(), Some("json"));
    assert!(one(params, "t").is_some() && one(params, "s").is_some());
    assert_eq!(one(params, "p"), None, "the password is never sent");
    assert!(
        !params.iter().any(|(_, v)| v.contains(PASSWORD)),
        "{params:?}"
    );
}

#[tokio::test]
async fn form_post_servers_get_every_write_as_a_form_post_with_nothing_in_the_url() {
    let (server, client) = start(true).await;
    mount_any(&server, "createPlaylist", ok(playlist_body("new1", &[]))).await;
    mount_any(&server, "updatePlaylist", ok(json!({}))).await;
    mount_any(&server, "deletePlaylist", ok(json!({}))).await;
    assert_eq!(client.transport().await.unwrap(), Transport::FormPost);

    let songs = vec!["a&b".to_owned(), "c d".to_owned(), "a&b".to_owned()];
    let created = client.create_playlist("Mix & Match", &songs).await.unwrap();
    assert_eq!(created.as_deref(), Some("new1"));
    client.replace_playlist("p1", &songs).await.unwrap();
    // A clone shares the detected transport.
    let clone = client.clone();
    clone.add_to_playlist("p1", &songs[..2]).await.unwrap();
    clone.rename_playlist("p1", "Néw name").await.unwrap();
    clone.delete_playlist("p1").await.unwrap();

    assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 1);
    let writes: Vec<Request> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| !r.url.path().ends_with("getOpenSubsonicExtensions"))
        .collect();
    assert_eq!(writes.len(), 5);
    for request in &writes {
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.query(), None, "nothing in the URL");
        assert_eq!(
            request.headers.get("content-type").unwrap(),
            "application/x-www-form-urlencoded"
        );
        assert_signed(&params(request));
    }
    let p: Vec<_> = writes.iter().map(params).collect();
    assert_eq!(writes[0].url.path(), "/rest/createPlaylist");
    assert_eq!(one(&p[0], "name").as_deref(), Some("Mix & Match"));
    assert_eq!(values(&p[0], "songId"), songs, "order and duplicates kept");
    assert_eq!(one(&p[0], "playlistId"), None);
    assert_eq!(writes[1].url.path(), "/rest/createPlaylist");
    assert_eq!(one(&p[1], "playlistId").as_deref(), Some("p1"));
    assert_eq!(one(&p[1], "name"), None);
    assert_eq!(values(&p[1], "songId"), songs);
    assert_eq!(writes[2].url.path(), "/rest/updatePlaylist");
    assert_eq!(one(&p[2], "playlistId").as_deref(), Some("p1"));
    assert_eq!(values(&p[2], "songIdToAdd"), songs[..2]);
    assert_eq!(one(&p[3], "name").as_deref(), Some("Néw name"));
    assert_eq!(one(&p[3], "playlistId").as_deref(), Some("p1"));
    assert_eq!(writes[4].url.path(), "/rest/deletePlaylist");
    assert_eq!(one(&p[4], "id").as_deref(), Some("p1"));
    // An empty replace is refused unsent: Navidrome would ignore it.
    let error = client.replace_playlist("p1", &[]).await.unwrap_err();
    assert!(matches!(error, Error::EmptyReplace), "{error:?}");
    assert_eq!(sent(&server, "createPlaylist").await.len(), 2);
}

#[tokio::test]
async fn servers_without_form_post_get_writes_as_get_and_are_asked_once() {
    for extensions in [
        ResponseTemplate::new(404),
        failed(0, "unknown endpoint"),
        ok(json!({ "openSubsonicExtensions": [{ "name": "songLyrics" }] })),
        ok(json!({ "openSubsonicExtensions": "formPost" })),
        ok(json!({ "openSubsonicExtensions": [{ "name": ["formPost"] }] })),
    ] {
        let server = MockServer::start().await;
        mount_any(&server, "getOpenSubsonicExtensions", extensions).await;
        mount_any(&server, "updatePlaylist", ok(json!({}))).await;
        let client = client_at(&server.uri());
        assert_eq!(client.transport().await.unwrap(), Transport::Get);
        client.rename_playlist("p1", "New").await.unwrap();
        client
            .add_to_playlist("p1", &["s1".to_owned(), "s2".to_owned()])
            .await
            .unwrap();
        assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 1);
        let writes = sent(&server, "updatePlaylist").await;
        assert_eq!(writes.len(), 2);
        for request in &writes {
            assert_eq!(request.method.as_str(), "GET");
            assert!(request.body.is_empty());
            assert_signed(&params(request));
        }
        assert_eq!(one(&params(&writes[0]), "name").as_deref(), Some("New"));
        assert_eq!(values(&params(&writes[1]), "songIdToAdd"), ["s1", "s2"]);
    }
}

#[tokio::test]
async fn a_network_failure_during_detection_fails_the_write_and_is_not_remembered() {
    // Nothing listens on this port once the listener is gone. (Wiremock
    // servers are pooled and keep listening after a drop.)
    let base = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let client = client_at(&base);
    let error = client.delete_playlist("p1").await.unwrap_err();
    assert!(matches!(error, Error::Network(_)), "{error:?}");
    let text = format!("{error} {error:?} {}", error.detail());
    assert!(
        !text.contains(PASSWORD) && !text.contains("rest/"),
        "{text}"
    );
    // Still unknown: a later write asks again (and fails again here).
    assert!(matches!(client.transport().await, Err(Error::Network(_))));
}

#[tokio::test]
async fn a_server_error_during_detection_is_asked_again_next_time() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getOpenSubsonicExtensions"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_any(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({ "openSubsonicExtensions": [{ "name": "formPost" }] })),
    )
    .await;
    mount_any(&server, "deletePlaylist", ok(json!({}))).await;
    let client = client_at(&server.uri());
    let error = client.delete_playlist("p1").await.unwrap_err();
    assert!(matches!(error, Error::Http(503)), "{error:?}");
    assert!(sent(&server, "deletePlaylist").await.is_empty());
    client.delete_playlist("p1").await.unwrap();
    assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 2);
    assert_eq!(
        sent(&server, "deletePlaylist").await[0].method.as_str(),
        "POST"
    );
}

#[tokio::test]
async fn get_writes_are_batched_under_8_kib_in_order() {
    let (server, client) = start(false).await;
    mount_any(&server, "updatePlaylist", ok(json!({}))).await;
    let songs = ids(1_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Add {
            playlist_id: "p1".into(),
            songs: songs.clone(),
        },
    )
    .await;
    assert!(
        matches!(result, EditResult::Done { created: None }),
        "{result:?}"
    );
    let writes = sent(&server, "updatePlaylist").await;
    assert!(writes.len() > 1, "batched");
    let mut all = Vec::new();
    for request in &writes {
        assert_eq!(request.method.as_str(), "GET");
        assert!(
            request.url.as_str().len() <= MAX_GET_URL_BYTES,
            "{} bytes",
            request.url.as_str().len()
        );
        let p = params(request);
        assert_eq!(one(&p, "playlistId").as_deref(), Some("p1"));
        all.extend(values(&p, "songIdToAdd"));
    }
    assert_eq!(all, songs, "every song once, in order");
    // Batches are filled: the next song would not have fitted.
    let first = writes[0].url.as_str().len();
    assert!(
        first + "&songIdToAdd=".len() + 22 > MAX_GET_URL_BYTES,
        "{first}"
    );
}

#[tokio::test]
async fn form_post_writes_of_a_full_queue_take_one_request() {
    let (server, client) = start(true).await;
    mount_any(&server, "createPlaylist", ok(playlist_body("new1", &[]))).await;
    let songs = ids(10_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Create {
            name: "Queue".into(),
            songs: songs.clone(),
        },
    )
    .await;
    assert!(matches!(result, EditResult::Done { created: Some(ref id) } if id == "new1"));
    let writes = sent(&server, "createPlaylist").await;
    assert_eq!(writes.len(), 1);
    assert!(writes[0].body.len() <= MAX_POST_BODY_BYTES);
    assert_eq!(values(&params(&writes[0]), "songId"), songs);
}

#[tokio::test]
async fn create_then_add_reports_partial_failure_honestly() {
    let (server, client) = start(false).await;
    mount_any(&server, "createPlaylist", ok(playlist_body("new1", &[]))).await;
    // The first add succeeds, the second fails.
    Mock::given(method("GET"))
        .and(path("/rest/updatePlaylist"))
        .respond_with(ok(json!({})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_any(&server, "updatePlaylist", failed(0, "disk full")).await;
    let songs = ids(1_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Create {
            name: "Big".into(),
            songs: songs.clone(),
        },
    )
    .await;
    let creates = sent(&server, "createPlaylist").await;
    let adds = sent(&server, "updatePlaylist").await;
    assert_eq!(creates.len(), 1);
    assert_eq!(adds.len(), 2, "stops at the first failure");
    let first = values(&params(&creates[0]), "songId").len();
    let second = values(&params(&adds[0]), "songIdToAdd").len();
    assert_eq!(
        one(&params(&adds[0]), "playlistId").as_deref(),
        Some("new1")
    );
    match result {
        EditResult::Partial {
            created,
            added,
            error,
        } => {
            assert_eq!(created.as_deref(), Some("new1"));
            assert_eq!(added, first + second);
            assert!(added < songs.len());
            assert!(error.to_string().contains("disk full"), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_create_whose_reply_has_no_id_stops_after_the_first_request() {
    let (server, client) = start(false).await;
    mount_any(&server, "createPlaylist", ok(json!({}))).await;
    let songs = ids(1_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Create {
            name: "Old server".into(),
            songs,
        },
    )
    .await;
    assert!(
        matches!(
            result,
            EditResult::Partial {
                created: None,
                added,
                ..
            } if added > 0
        ),
        "{result:?}"
    );
    assert!(sent(&server, "updatePlaylist").await.is_empty());
}

#[tokio::test]
async fn a_replace_that_does_not_fit_one_get_is_refused_before_anything_is_sent() {
    let (server, client) = start(false).await;
    mount_any(&server, "getPlaylist", ok(playlist_body("p1", &[]))).await;
    mount_any(&server, "createPlaylist", ok(playlist_body("p1", &[]))).await;
    let songs = ids(1_000, 22);
    let error = client.replace_playlist("p1", &songs).await.unwrap_err();
    assert!(matches!(error, Error::RequestTooLarge), "{error:?}");
    let result = edit::run(
        &client,
        &EditOp::Replace {
            playlist_id: "p1".into(),
            expected: songs.clone(),
            songs,
            hidden: 0,
        },
    )
    .await;
    assert!(
        matches!(result, EditResult::Failed(Error::RequestTooLarge)),
        "{result:?}"
    );
    assert!(sent(&server, "createPlaylist").await.is_empty());
    assert!(sent(&server, "getPlaylist").await.is_empty());
    // A form-post server takes it in one request.
    let (post_server, post_client) = start(true).await;
    mount_any(&post_server, "createPlaylist", ok(playlist_body("p1", &[]))).await;
    post_client
        .replace_playlist("p1", &ids(1_000, 22))
        .await
        .unwrap();
    assert_eq!(sent(&post_server, "createPlaylist").await.len(), 1);
}

#[tokio::test]
async fn index_edits_check_the_playlist_first_and_refuse_a_stale_one() {
    let shown = vec!["s1".to_owned(), "s2".to_owned(), "s1".to_owned()];
    for (server_has, stale) in [
        (shown.clone(), false),
        (vec!["s1".into(), "s1".into(), "s2".into()], true),
        (vec!["s1".into(), "s2".into()], true),
        (
            vec!["s1".into(), "s2".into(), "s1".into(), "s3".into()],
            true,
        ),
    ] {
        let (server, client) = start(true).await;
        mount_any(&server, "getPlaylist", ok(playlist_body("p1", &server_has))).await;
        mount_any(&server, "updatePlaylist", ok(json!({}))).await;
        mount_any(&server, "createPlaylist", ok(playlist_body("p1", &[]))).await;
        let remove = edit::run(
            &client,
            &EditOp::Remove {
                playlist_id: "p1".into(),
                index: 2,
                expected: shown.clone(),
                hidden: 0,
            },
        )
        .await;
        let reordered = vec!["s2".to_owned(), "s1".to_owned(), "s1".to_owned()];
        let replace = edit::run(
            &client,
            &EditOp::Replace {
                playlist_id: "p1".into(),
                expected: shown.clone(),
                songs: reordered.clone(),
                hidden: 0,
            },
        )
        .await;
        let fetches = sent(&server, "getPlaylist").await;
        assert_eq!(fetches.len(), 2, "each checks first");
        assert_eq!(one(&params(&fetches[0]), "id").as_deref(), Some("p1"));
        // Removal is never by index (see `index_removal_is_a_replace`).
        assert!(sent(&server, "updatePlaylist").await.is_empty());
        let replaces = sent(&server, "createPlaylist").await;
        if stale {
            assert!(matches!(remove, EditResult::Stale), "{remove:?}");
            assert!(matches!(replace, EditResult::Stale), "{replace:?}");
            assert!(replaces.is_empty());
        } else {
            assert!(matches!(remove, EditResult::Done { .. }), "{remove:?}");
            assert!(matches!(replace, EditResult::Done { .. }), "{replace:?}");
            assert_eq!(replaces.len(), 2);
            // The removal: the shown list without entry 2, as a replace.
            assert_eq!(values(&params(&replaces[0]), "songId"), ["s1", "s2"]);
            assert_eq!(values(&params(&replaces[1]), "songId"), reordered);
            for replace in &replaces {
                assert_eq!(one(&params(replace), "playlistId").as_deref(), Some("p1"));
                assert_eq!(one(&params(replace), "songIndexToRemove"), None);
            }
        }
    }
}

#[tokio::test]
async fn an_entry_without_a_usable_id_makes_the_playlist_stale() {
    let (server, client) = start(true).await;
    mount_any(
        &server,
        "getPlaylist",
        ok(json!({ "playlist": { "id": "p1", "entry": [
            { "id": "s1" }, { "title": "no id" }, { "id": "s2" }
        ]}})),
    )
    .await;
    let result = edit::run(
        &client,
        &EditOp::Remove {
            playlist_id: "p1".into(),
            index: 0,
            expected: vec!["s1".into(), "s2".into()],
            hidden: 0,
        },
    )
    .await;
    assert!(matches!(result, EditResult::Stale), "{result:?}");
    assert!(sent(&server, "updatePlaylist").await.is_empty());
}

#[tokio::test]
async fn write_errors_are_short_and_carry_no_url_token_or_password() {
    for (form_post, response) in [
        (true, failed(50, "user not authorized")),
        (false, failed(50, "user not authorized")),
        (true, ResponseTemplate::new(500)),
        (
            false,
            ResponseTemplate::new(302).insert_header("location", "https://evil.example/"),
        ),
        (true, ResponseTemplate::new(200).set_body_string("<html>")),
    ] {
        let (server, client) = start(form_post).await;
        mount_any(&server, "deletePlaylist", response).await;
        let error = client.delete_playlist("p1").await.unwrap_err();
        let requests = sent(&server, "deletePlaylist").await;
        assert_eq!(requests.len(), 1, "redirects are not followed");
        let p = params(&requests[0]);
        let token = one(&p, "t").unwrap();
        let salt = one(&p, "s").unwrap();
        let text = format!("{error} | {error:?} | {}", error.detail());
        for secret in [
            PASSWORD,
            token.as_str(),
            salt.as_str(),
            "rest/",
            "127.0.0.1",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        match error {
            Error::Server { code, .. } => assert_eq!(code, 50),
            Error::Http(500) | Error::Redirect(302) | Error::Decode(_) => {}
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn a_cancelled_edit_still_answers_once() {
    let (_server, client) = start(true).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = EditRequest {
        generation: Generation(3),
        id: EditId(9),
        op: EditOp::Delete {
            playlist_id: "p1".into(),
        },
    };
    let task = edit::spawn_edit(client, request, tx);
    task.abort();
    let response = rx.recv().await.unwrap();
    assert_eq!(response.generation, Generation(3));
    assert_eq!(response.id, EditId(9));
    assert!(matches!(
        response.result,
        EditResult::Failed(Error::Aborted)
    ));
    assert!(rx.recv().await.is_none(), "exactly once");
}

#[tokio::test]
async fn a_spawned_edit_answers_with_its_result() {
    let (server, client) = start(true).await;
    mount_any(&server, "deletePlaylist", ok(json!({}))).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = EditRequest {
        generation: Generation(1),
        id: EditId(1),
        op: EditOp::Delete {
            playlist_id: "p1".into(),
        },
    };
    edit::spawn_edit(client, request, tx);
    let response = rx.recv().await.unwrap();
    assert!(matches!(
        response.result,
        EditResult::Done { created: None }
    ));
    assert!(rx.recv().await.is_none(), "exactly once");
}

#[tokio::test]
async fn index_removal_is_a_replace_and_never_empties_a_playlist() {
    let (server, client) = start(false).await;
    mount_any(
        &server,
        "getPlaylist",
        ok(playlist_body("p1", &["s1".into()])),
    )
    .await;
    mount_any(&server, "createPlaylist", ok(playlist_body("p1", &[]))).await;
    // Removing the only song would be an empty replace, which Navidrome
    // ignores: refused before anything is sent.
    let result = edit::run(
        &client,
        &EditOp::Remove {
            playlist_id: "p1".into(),
            index: 0,
            expected: vec!["s1".into()],
            hidden: 0,
        },
    )
    .await;
    assert!(
        matches!(result, EditResult::Failed(Error::EmptyReplace)),
        "{result:?}"
    );
    // An index past the shown list is stale.
    let result = edit::run(
        &client,
        &EditOp::Remove {
            playlist_id: "p1".into(),
            index: 3,
            expected: vec!["s1".into(), "s2".into()],
            hidden: 0,
        },
    )
    .await;
    assert!(matches!(result, EditResult::Stale), "{result:?}");
    assert!(sent(&server, "getPlaylist").await.is_empty());
    assert!(sent(&server, "createPlaylist").await.is_empty());
    // A removal that does not fit one GET is refused like a reorder.
    let songs = ids(1_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Remove {
            playlist_id: "p1".into(),
            index: 0,
            expected: songs,
            hidden: 0,
        },
    )
    .await;
    assert!(
        matches!(result, EditResult::Failed(Error::RequestTooLarge)),
        "{result:?}"
    );
    assert!(sent(&server, "createPlaylist").await.is_empty());
}

#[tokio::test]
async fn a_batched_add_that_fails_part_way_reports_what_was_added() {
    let (server, client) = start(false).await;
    Mock::given(path("/rest/updatePlaylist"))
        .respond_with(ok(json!({})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_any(&server, "updatePlaylist", ResponseTemplate::new(503)).await;
    let songs = ids(1_000, 22);
    let result = edit::run(
        &client,
        &EditOp::Add {
            playlist_id: "p1".into(),
            songs,
        },
    )
    .await;
    let adds = sent(&server, "updatePlaylist").await;
    assert_eq!(adds.len(), 2, "stops at the first failure");
    let first = values(&params(&adds[0]), "songIdToAdd").len();
    match result {
        EditResult::Partial {
            created: None,
            added,
            error: Error::Http(503),
        } => assert_eq!(added, first),
        other => panic!("{other:?}"),
    }
    // A failure of the very first batch changed nothing.
    let (server, client) = start(false).await;
    mount_any(&server, "updatePlaylist", ResponseTemplate::new(503)).await;
    let result = edit::run(
        &client,
        &EditOp::Add {
            playlist_id: "p1".into(),
            songs: ids(1_000, 22),
        },
    )
    .await;
    assert!(
        matches!(result, EditResult::Failed(Error::Http(503))),
        "{result:?}"
    );
    assert_eq!(sent(&server, "updatePlaylist").await.len(), 1);
}

#[tokio::test]
async fn a_post_answered_with_a_307_or_308_is_not_followed() {
    for status in [307, 308] {
        let (server, client) = start(true).await;
        mount_any(
            &server,
            "deletePlaylist",
            ResponseTemplate::new(status).insert_header("location", "/rest/elsewhere"),
        )
        .await;
        mount_any(&server, "elsewhere", ok(json!({}))).await;
        let error = client.delete_playlist("p1").await.unwrap_err();
        assert!(
            matches!(error, Error::Redirect(s) if s == status),
            "{error:?}"
        );
        let requests = server.received_requests().await.unwrap();
        let writes: Vec<&Request> = requests
            .iter()
            .filter(|r| !r.url.path().ends_with("getOpenSubsonicExtensions"))
            .collect();
        assert_eq!(writes.len(), 1, "no second request, so no body re-sent");
        assert_eq!(writes[0].method.as_str(), "POST");
    }
}

#[tokio::test]
async fn only_a_definite_answer_about_extensions_is_remembered() {
    // Remembered as "no formPost": a Subsonic error, 400, 404, 405, 410,
    // 501.
    for answer in [
        failed(0, "unknown"),
        ResponseTemplate::new(400),
        ResponseTemplate::new(404),
        ResponseTemplate::new(405),
        ResponseTemplate::new(410),
        ResponseTemplate::new(501),
    ] {
        let server = MockServer::start().await;
        mount_any(&server, "getOpenSubsonicExtensions", answer).await;
        mount_any(&server, "deletePlaylist", ok(json!({}))).await;
        let client = client_at(&server.uri());
        client.delete_playlist("p1").await.unwrap();
        client.delete_playlist("p1").await.unwrap();
        assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 1);
        assert!(
            sent(&server, "deletePlaylist")
                .await
                .iter()
                .all(|r| r.method.as_str() == "GET")
        );
    }
    // Not remembered, and the write fails: rejected credentials, 401, 403,
    // 408, 429, a server error, an unreadable body.
    for answer in [
        failed(40, "Wrong username or password"),
        ResponseTemplate::new(401),
        ResponseTemplate::new(403),
        ResponseTemplate::new(408),
        ResponseTemplate::new(429),
        ResponseTemplate::new(500),
        ResponseTemplate::new(200).set_body_string("<html>"),
    ] {
        let server = MockServer::start().await;
        mount_any(&server, "getOpenSubsonicExtensions", answer).await;
        mount_any(&server, "deletePlaylist", ok(json!({}))).await;
        let client = client_at(&server.uri());
        assert!(client.delete_playlist("p1").await.is_err());
        assert!(client.delete_playlist("p1").await.is_err());
        assert_eq!(sent(&server, "getOpenSubsonicExtensions").await.len(), 2);
        assert!(sent(&server, "deletePlaylist").await.is_empty());
    }
}

/// A playlist whose stored `songCount` is `count` while `songs` are
/// returned (Navidrome hides missing files and other libraries).
fn counted_body(songs: &[String], count: Option<u64>) -> Value {
    let mut body = playlist_body("p1", songs);
    if let Some(count) = count {
        body["playlist"]["songCount"] = json!(count);
    }
    body
}

#[tokio::test]
async fn the_stored_song_count_reveals_hidden_entries() {
    let (server, client) = start(true).await;
    let songs = vec!["s1".to_owned(), "s2".to_owned()];
    mount_any(&server, "getPlaylist", ok(counted_body(&songs, Some(5)))).await;
    let playlist = client.playlist("p1").await.unwrap();
    assert_eq!(playlist.song_count, Some(5));
    assert_eq!(playlist.hidden_entries(), Hidden::Some(3));
}

#[tokio::test]
async fn more_hidden_entries_than_confirmed_on_the_refetch_is_stale() {
    let shown = vec!["s1".to_owned(), "s2".to_owned(), "s3".to_owned()];
    // (songCount on the server, hidden entries the user confirmed, stale)
    for (count, confirmed, stale) in [
        (Some(5), 2, false),
        (Some(5), 3, false),
        (Some(6), 2, true),
        (Some(3), 0, false),
        // Fewer stored than returned: suspect, so hidden can't be ruled out.
        (Some(2), 0, true),
        (Some(2), usize::MAX, false),
        // No songCount: nothing to compare, as before.
        (None, 0, false),
    ] {
        let (server, client) = start(true).await;
        mount_any(&server, "getPlaylist", ok(counted_body(&shown, count))).await;
        mount_any(&server, "createPlaylist", ok(playlist_body("p1", &[]))).await;
        let remove = edit::run(
            &client,
            &EditOp::Remove {
                playlist_id: "p1".into(),
                index: 0,
                expected: shown.clone(),
                hidden: confirmed,
            },
        )
        .await;
        let replace = edit::run(
            &client,
            &EditOp::Replace {
                playlist_id: "p1".into(),
                expected: shown.clone(),
                songs: vec!["s3".into(), "s2".into(), "s1".into()],
                hidden: confirmed,
            },
        )
        .await;
        let writes = sent(&server, "createPlaylist").await;
        if stale {
            assert!(matches!(remove, EditResult::Stale), "{count:?} {remove:?}");
            assert!(
                matches!(replace, EditResult::Stale),
                "{count:?} {replace:?}"
            );
            assert!(writes.is_empty(), "{count:?}");
        } else {
            assert!(
                matches!(remove, EditResult::Done { .. }),
                "{count:?} {remove:?}"
            );
            assert!(
                matches!(replace, EditResult::Done { .. }),
                "{count:?} {replace:?}"
            );
            assert_eq!(writes.len(), 2, "{count:?}");
        }
    }
}
