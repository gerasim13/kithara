use std::{collections::HashMap, iter, num::NonZeroU16};

use bytes::Bytes;
use kithara_net::{Headers, NetError, mock::NetMock};
#[cfg(not(target_arch = "wasm32"))]
use kithara_platform::time::Duration;
use kithara_test_utils::kithara;
use serde_json::Value;
use unimock::{MockFn, Unimock, matching};
use url::Url;

use crate::{Client, Config, Error, PlaylistId, TrackId};

fn identity() -> Config {
    Config {
        user_agent: "synthetic-agent".to_owned(),
        auth_token: "synthetic-test-token".to_owned(),
    }
}

fn mock_client(net: Unimock) -> Client<Unimock> {
    Client::with_transports(net.clone(), net, &identity())
}

fn reply(body: impl Into<Bytes>) -> Unimock {
    Unimock::new(
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Ok(body.into())),
    )
}

fn failing(error: NetError) -> Unimock {
    Unimock::new(
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Err(error)),
    )
}

/// The section's identity and the body type are the only headers a catalogue
/// request carries.
#[kithara::test]
async fn search_is_typed_and_carries_exactly_the_section_identity() {
    fn expected() -> Headers {
        Headers::from(HashMap::from([
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), "synthetic-agent".to_owned()),
            ("X-Auth-Token".to_owned(), "synthetic-test-token".to_owned()),
        ]))
    }
    let net = Unimock::new(
        NetMock::post_bytes
            .next_call(matching!((url, body, headers) if
                url.as_str() == "https://zvuk.com/api/v1/graphql/" &&
                serde_json::from_slice::<Value>(body).unwrap()["variables"]["query"] == "Mozart" &&
                headers.as_ref() == Some(&expected())
            ))
            .returns(Ok(Bytes::from_static(include_bytes!(
                "../../tests/fixtures/search.json"
            )))),
    );
    let tracks = mock_client(net).search("Mozart").await.unwrap().tracks;
    assert_eq!(tracks[0].title, "Amber Field");
    assert_eq!(tracks[0].duration, 128);
    assert_eq!(
        tracks[0]
            .artists
            .iter()
            .map(|artist| artist.title.as_str())
            .collect::<Vec<_>>(),
        ["Ana Vale", "Ben Ore"]
    );
    assert_eq!(
        tracks
            .iter()
            .map(|track| (track.album(), track.artwork()))
            .collect::<Vec<_>>(),
        [
            (
                Some("First Light"),
                Some("https://covers.example.invalid/{size}/1000.jpg")
            ),
            (Some("Second Wind"), None),
            (None, None),
            (None, None),
            (
                Some("Fifth Season"),
                Some("https://covers.example.invalid/{size}/1010.jpg")
            ),
        ]
    );
}

/// Whether a catalogue request asks for every field its page total comes from.
fn asks_for_its_total(request: &[u8]) -> bool {
    let request = String::from_utf8_lossy(request);
    let fields: &[&str] = if request.contains("KitharaSearch") {
        &["page { total }"]
    } else if request.contains("KitharaLiked") {
        &["collectionCount { tracks }", "page { hasNextPage }"]
    } else {
        &["trackCount"]
    };
    fields.iter().all(|field| request.contains(field))
}

/// Each page asks the service for its total and reports it, inferring one only
/// from a liked page that says it is the last.
#[kithara::test]
async fn each_page_reports_the_total_the_service_establishes() {
    let track = r#"{"id":"1000","title":"Amber Field","duration":128,"artists":[],"release":null,"collectionItemData":null}"#;
    let liked_page = |count: &str, more: bool| {
        format!(
            r#"{{"data":{{"collectionCount":{{"tracks":{count}}},"paginatedCollection":{{"tracks":{{"items":[{track}],"page":{{"hasNextPage":{more}}}}}}}}}}}"#
        )
    };
    let search_total = |total: &str| {
        format!(r#"{{"data":{{"search":{{"tracks":{{"items":[],"page":{{"total":{total}}}}}}}}}}}"#)
    };
    let playlist_page = |id: &str| {
        format!(
            r#"{{"data":{{"playlists":[{{"id":"{id}","trackCount":215,"tracks":[{track}]}}]}}}}"#
        )
    };
    for (operation, body, expected) in [
        (
            "search",
            include_str!("../../tests/fixtures/search.json").to_owned(),
            Ok(Some(120)),
        ),
        ("search", search_total("-1"), Err("invalid catalogue total")),
        ("liked", liked_page("153", true), Ok(Some(153))),
        ("liked", liked_page("null", false), Ok(Some(1))),
        ("liked", liked_page("null", true), Ok(None)),
        ("playlist", playlist_page("1023"), Ok(Some(215))),
        (
            "playlist",
            playlist_page("1024"),
            Err("playlist identity does not match request"),
        ),
    ] {
        let client = mock_client(Unimock::new(
            NetMock::post_bytes
                .next_call(matching!((_, request, _) if asks_for_its_total(request)))
                .returns(Ok(Bytes::from(body.clone()))),
        ));
        let page = match operation {
            "search" => client.search("query").await,
            "liked" => client.liked_tracks().await,
            _ => client.playlist_tracks(&PlaylistId("1023".into())).await,
        };
        let total = match page {
            Ok(page) => Ok(page.total),
            Err(Error::Protocol(reason)) => Err(reason),
            Err(other) => panic!("{body}: {other:?}"),
        };
        assert_eq!(total, expected, "{body}");
    }
}

/// A stream batch keeps every row's stream, expiry and playable URL, or fails
/// whole when a row or the batch breaks the request's shape.
#[kithara::test]
async fn a_stream_batch_resolves_every_row_or_fails_whole() {
    let ids: Vec<TrackId> = ["1000", "1003", "1006", "1008", "1010"]
        .map(|id| TrackId(id.into()))
        .into();
    let streams = mock_client(reply(include_str!("../../tests/fixtures/streams.json")))
        .streams(&ids)
        .await
        .unwrap();
    assert_eq!(
        streams.iter().map(|row| &row.id).collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    assert!(streams.iter().all(|row| {
        row.stream_v3
            .as_ref()
            .is_some_and(|stream| stream.hls.is_some() && stream.expire.is_some())
    }));
    assert!(
        mock_client(Unimock::new(()))
            .streams(&[])
            .await
            .unwrap()
            .is_empty()
    );

    let url = "https://media.example.invalid/1000/master.m3u8";
    let expire = "2026-10-04T15:00:00+00:00";
    let batch = |rows: &str| format!(r#"{{"data":{{"mediaContents":[{rows}]}}}}"#);
    for (body, expected) in [
        (batch(r#"{"id":"1000","streamV3":null}"#), Ok(None)),
        (
            batch(&format!(
                r#"{{"id":"1000","streamV3":{{"hls":null,"expire":"{expire}"}}}}"#
            )),
            Ok(Some((None, Some(expire)))),
        ),
        (
            batch(r#"{"id":"1000","streamV3":{"hls":"","expire":null}}"#),
            Ok(Some((None, None))),
        ),
        (
            batch(&format!(
                r#"{{"id":"1000","streamV3":{{"hls":"{url}","expire":null}}}}"#
            )),
            Ok(Some((Some(url), None))),
        ),
        (
            batch(r#"{"id":"1000","streamV3":{"hls":"not a URL","expire":null}}"#),
            Err(()),
        ),
        (
            batch(r#"{"id":"1000","streamV3":{"hls":"file:///private/media","expire":null}}"#),
            Err(()),
        ),
        (batch(r#"{"id":"1000"}"#), Err(())),
        (batch(""), Err(())),
        ("{}".to_owned(), Err(())),
        ("not json".to_owned(), Err(())),
    ] {
        let resolved = mock_client(reply(body.clone()))
            .streams(&[TrackId("1000".into())])
            .await;
        let resolved = match &resolved {
            Ok(rows) => Ok(rows[0].stream_v3.as_ref().map(|stream| {
                (
                    stream.hls.as_ref().map(Url::as_str),
                    stream.expire.as_deref(),
                )
            })),
            Err(Error::Protocol(_)) => Err(()),
            Err(other) => panic!("{body}: {other:?}"),
        };
        assert_eq!(resolved, expected, "{body}");
    }
}

/// A failure is worded by its cause: structured errors win over the reply's
/// data, only HTTP 401 rejects the token, a status keeps its body out, and a
/// transport failure keeps its source.
#[kithara::test]
async fn a_failure_is_worded_by_its_cause() {
    let status = |status: u16| {
        failing(NetError::Status {
            status: NonZeroU16::new(status).unwrap(),
            url: None,
            body: Some("Synthetic sensitive body".into()),
        })
    };
    for (net, expected) in [
        (
            reply(
                r#"{"data":{"search":{"tracks":{"items":[]}}},"errors":[{"message":"Synthetic failure"}]}"#,
            ),
            "Synthetic failure",
        ),
        (
            reply(include_str!("../../tests/fixtures/graphql_error.json")),
            "Synthetic service error",
        ),
        (status(401), "Zvuk rejected the authentication token"),
        (status(403), "Zvuk HTTP 403"),
        (status(500), "Zvuk HTTP 500"),
        (
            failing(NetError::Network("Synthetic disconnect".into())),
            "Zvuk HTTP transport failed: Network error: Synthetic disconnect",
        ),
    ] {
        let error = mock_client(net).search("query").await.unwrap_err();
        let causes = iter::successors(Some(&error as &dyn std::error::Error), |cause| {
            cause.source()
        });
        assert_eq!(
            causes
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(": "),
            expected
        );
        assert!(!format!("{error:?}").contains("Synthetic sensitive body"));
    }
}

/// A reaction goes through the mutation transport alone and succeeds only on
/// the service's confirmation of that exact mutation.
#[kithara::test]
async fn reads_and_reactions_use_their_own_transports_and_a_reaction_needs_its_confirmation() {
    async fn react(mutations: Unimock, liked: bool) -> Result<(), Error> {
        Client::with_transports(Unimock::new(()), mutations, &identity())
            .set_liked(&TrackId("1000".into()), liked)
            .await
    }
    let reads = Client::with_transports(
        reply(include_str!("../../tests/fixtures/search.json")),
        Unimock::new(()),
        &identity(),
    );
    assert_eq!(reads.search("needle").await.unwrap().tracks.len(), 5);
    let like = Unimock::new(
        NetMock::post_bytes
            .next_call(matching!((_, request, _) if String::from_utf8_lossy(request).contains(r#"addItem(id: \"1000\""#)))
            .returns(Ok(Bytes::from_static(include_bytes!("../../tests/fixtures/like.json")))),
    );
    assert!(react(like, true).await.is_ok());
    let unlike = Unimock::new(
        NetMock::post_bytes
            .next_call(matching!((_, request, _) if String::from_utf8_lossy(request).contains(r#"removeItem(id: \"1000\""#)))
            .returns(Ok(Bytes::from_static(include_bytes!("../../tests/fixtures/unlike.json")))),
    );
    assert!(react(unlike, false).await.is_ok());
    assert!(matches!(
        react(reply(r#"{"data":{"collection":{}}}"#), true).await,
        Err(Error::Protocol(_))
    ));
}

/// A reaction whose reply is lost is sent once, while a lost read is resent
/// by the transport's retries.
#[kithara::test(native, tokio, timeout(Duration::from_secs(2)))]
async fn a_lost_reaction_is_sent_once_and_a_lost_read_is_resent() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use axum::{Router, http::StatusCode};
    use kithara_net::{HttpClient, NetOptions, RetryPolicy};
    use kithara_test_utils::{TestHttpServer, bufpool::pools, cancel_token};

    let requests = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&requests);
    let server = TestHttpServer::new(Router::new().fallback(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        async { StatusCode::SERVICE_UNAVAILABLE }
    }))
    .await;
    let options = NetOptions::builder()
        .retry_policy(
            RetryPolicy::builder()
                .max_retries(2)
                .base_delay(Duration::from_millis(1))
                .build(),
        )
        .build();
    let mut client = Client::new(
        HttpClient::new(options, pools(), cancel_token()),
        &identity(),
    );
    client.endpoint = server.url("/");

    assert!(
        client
            .set_liked(&TrackId("1000".into()), true)
            .await
            .is_err()
    );
    assert_eq!(requests.swap(0, Ordering::SeqCst), 1);
    assert!(client.search("query").await.is_err());
    assert!(requests.load(Ordering::SeqCst) > 1);
}
