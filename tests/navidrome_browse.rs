//! The background fetch task against a local `wiremock` server: a request
//! goes in, a response event with the mapped result comes out. No test
//! contacts a real server.

use std::sync::Arc;
use std::time::Duration;

use coda_player::navidrome::browse::{
    Content, FetchRequest, FetchResponse, RequestId, Resource, spawn_fetch,
};
use coda_player::navidrome::client::parse_base_url;
use coda_player::navidrome::{Client, Error, Password};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PASSWORD: &str = "s3cret-pass";

fn ok(body: Value) -> ResponseTemplate {
    let mut response = json!({ "status": "ok", "version": "1.16.1" });
    if let (Some(target), Value::Object(extra)) = (response.as_object_mut(), body) {
        target.extend(extra);
    }
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

async fn server() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::new(
        parse_base_url(&server.uri()).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    (server, client)
}

async fn run(client: &Client, id: u64, resource: Resource) -> FetchResponse {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = FetchRequest {
        id: RequestId(id),
        resource,
    };
    spawn_fetch(client.clone(), Arc::new(Semaphore::new(4)), request, tx);
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a response in time")
        .expect("a response")
}

#[tokio::test]
async fn each_resource_maps_to_its_endpoint_and_content() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtists"))
        .respond_with(ok(json!({ "artists": { "index": [
            { "name": "A", "artist": [{ "id": "ar1", "name": "ABBA" }] }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .and(query_param("id", "ar1"))
        .respond_with(ok(json!({ "artist": { "id": "ar1", "album": [
            { "id": "al1", "name": "Arrival" }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .and(query_param("id", "al1"))
        .respond_with(ok(json!({ "album": { "id": "al1", "song": [
            { "id": "s1", "title": "Dancing Queen" },
            { "id": "s2", "title": "Money, Money, Money" }
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .and(query_param("query", "queen"))
        .respond_with(ok(json!({ "searchResult3": {
            "song": [{ "id": "s1", "title": "Dancing Queen" }]
        }})))
        .mount(&server)
        .await;

    let response = run(&client, 1, Resource::Artists).await;
    assert_eq!(response.id, RequestId(1));
    assert_eq!(response.resource, Resource::Artists);
    let Ok(Content::Artists(artists)) = response.result else {
        panic!("{:?}", response.result);
    };
    assert_eq!(artists[0].name, "ABBA");

    let response = run(&client, 2, Resource::Artist("ar1".into())).await;
    assert_eq!(response.id, RequestId(2));
    let Ok(Content::Albums(albums)) = response.result else {
        panic!("{:?}", response.result);
    };
    assert_eq!(albums[0].id, "al1");

    let response = run(&client, 3, Resource::Album("al1".into())).await;
    let Ok(Content::Songs(songs)) = response.result else {
        panic!("{:?}", response.result);
    };
    let titles: Vec<&str> = songs.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Dancing Queen", "Money, Money, Money"]);

    let response = run(&client, 4, Resource::Search("queen".into())).await;
    assert_eq!(response.resource, Resource::Search("queen".into()));
    let Ok(Content::Search(results)) = response.result else {
        panic!("{:?}", response.result);
    };
    assert_eq!(results.tracks.len(), 1);
    assert!(results.artists.is_empty());
}

#[tokio::test]
async fn errors_map_to_short_text_without_urls_or_secrets() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subsonic-response": {
                "status": "failed", "version": "1.16.1",
                "error": { "code": 40, "message": "Wrong username or password" }
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;

    let cases = [
        (Resource::Album("x".into()), "server returned HTTP 500"),
        (Resource::Artist("x".into()), "authentication failed"),
        (
            Resource::Search("x".into()),
            "unexpected response from server",
        ),
    ];
    for (i, (resource, text)) in cases.into_iter().enumerate() {
        let response = run(&client, i as u64, resource.clone()).await;
        assert_eq!(response.resource, resource);
        let error = response.result.expect_err("an error");
        assert_eq!(error.to_string(), text);
    }

    // Connection refused: the network error carries no address or query.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);
    let closed = Client::new(
        parse_base_url(&format!("http://127.0.0.1:{port}")).unwrap(),
        "listener".into(),
        Password::new(PASSWORD.into()),
    )
    .unwrap();
    let response = run(&closed, 9, Resource::Artists).await;
    let error = response.result.expect_err("unreachable");
    assert!(matches!(error, Error::Network(_)), "{error:?}");
    let text = error.to_string();
    assert_eq!(text, "could not reach server");
    for secret in [PASSWORD, "t=", "s=", "127.0.0.1", port.as_str()] {
        assert!(
            !format!("{error} {}", error.detail()).contains(secret),
            "{secret}"
        );
    }
}

#[tokio::test]
async fn fetches_wait_for_a_permit() {
    let (server, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/rest/getArtists"))
        .respond_with(ok(json!({ "artists": { "index": [] } })))
        .mount(&server)
        .await;
    let limit = Arc::new(Semaphore::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_fetch(
        client,
        limit.clone(),
        FetchRequest {
            id: RequestId(1),
            resource: Resource::Artists,
        },
        tx,
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rx.try_recv().is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
    limit.add_permits(1);
    let response = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response.result, Ok(Content::Artists(ref a)) if a.is_empty()));
}

#[tokio::test]
async fn a_cancelled_fetch_still_answers_with_an_error() {
    let (_server, client) = server().await;
    // Cancelled before its first poll, and while waiting for a permit.
    for delay in [Duration::ZERO, Duration::from_millis(100)] {
        let limit = Arc::new(Semaphore::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = spawn_fetch(
            client.clone(),
            limit,
            FetchRequest {
                id: RequestId(7),
                resource: Resource::Album("al".into()),
            },
            tx,
        );
        tokio::time::sleep(delay).await;
        task.abort();
        let response = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .expect("an answer after cancellation");
        assert_eq!(response.id, RequestId(7));
        assert_eq!(response.resource, Resource::Album("al".into()));
        let error = response.result.expect_err("aborted");
        assert!(matches!(error, Error::Aborted), "{error:?}");
        assert_eq!(error.to_string(), "request did not complete");
        // Exactly one response.
        assert!(rx.recv().await.is_none());
    }
}
