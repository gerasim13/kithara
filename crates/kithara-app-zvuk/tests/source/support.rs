use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use kithara_app_library::{LibrarySource, PageStatus};
use kithara_app_zvuk::{Client, Config, Source};
use kithara_net::{ByteStream, Headers, Net, NetError, RangeSpec};
use kithara_platform::{
    CancelToken,
    time::{self, Duration},
    tokio::runtime::Handle,
};
use kithara_test_utils::wait_until;
use kithara_ui::{
    module::IconName,
    render::{TableValue, WriteValue},
};
use serde_json::{Value, json};
use unimock::Unimock;
use url::Url;

pub(super) fn search_reply() -> Bytes {
    Bytes::from_static(include_bytes!("../fixtures/search.json"))
}

pub(super) fn stream_reply() -> Bytes {
    Bytes::from_static(include_bytes!("../fixtures/streams.json"))
}

/// A liked page of the search fixture's tracks, each marked liked.
pub(super) fn liked_reply() -> Bytes {
    let mut tracks = search_tracks();
    for track in tracks
        .as_array_mut()
        .expect("the search fixture lists tracks")
    {
        track["collectionItemData"]["itemStatus"] = "liked".into();
    }
    Bytes::from(
        json!({"data": {
            "collectionCount": {"tracks": 153},
            "paginatedCollection": {"tracks": {"items": tracks, "page": {"hasNextPage": true}}},
        }})
        .to_string(),
    )
}

/// The first page of playlist 1023, the search fixture's tracks of 215.
pub(super) fn playlist_reply() -> Bytes {
    Bytes::from(
        json!({"data": {"playlists": [{"id": "1023", "trackCount": 215, "tracks": search_tracks()}]}})
            .to_string(),
    )
}

fn search_tracks() -> Value {
    let mut search: Value =
        serde_json::from_slice(&search_reply()).expect("the search fixture parses");
    search["data"]["search"]["tracks"]["items"].take()
}

/// Builds the source through its registration, worded by the captions it brings.
pub(super) fn registered<N: Net + Clone + 'static>(
    net: N,
    cancel: &CancelToken,
) -> Box<dyn LibrarySource> {
    let config: Config = serde_json::from_value(json!({ "user_agent": "", "auth_token": "" }))
        .expect("the section names both identity values");
    let registration = Source::registered(
        Client::with_transports(net.clone(), net, &config),
        Handle::current(),
        cancel.clone(),
    );
    let text = registration.page().texts[0];
    let words =
        kithara_ui::text::parse_text(text.text, &kithara_ui::ids::SourceUri(text.path.to_owned()))
            .expect("the source-owned caption document must parse");
    registration
        .build(&words)
        .expect("the source-owned caption document must contain every navigation label")
}

pub(super) fn source<N: Net + Clone + 'static>(net: N) -> Box<dyn LibrarySource> {
    registered(net, &kithara_test_utils::cancel_token())
}

/// Ticks the source until its page reaches `status`.
pub(super) async fn until(source: &mut dyn LibrarySource, status: PageStatus) {
    wait_until(
        Duration::from_secs(2),
        "the page reaches its status",
        || {
            source.tick();
            source.status() == status
        },
    )
    .await
    .expect("the source must reach the awaited page status");
}

/// A source showing the `needle` search's fixture rows.
pub(super) async fn loaded_search<N: Net + Clone + 'static>(net: N) -> Box<dyn LibrarySource> {
    let mut source = source(net);
    source.write("query", &WriteValue::Text("needle".into()));
    time::sleep(Duration::from_millis(300)).await;
    until(&mut *source, PageStatus::Ready).await;
    source
}

/// Starts a search for `query` and waits until its transport has finished,
/// leaving the completion queued for the next tick.
pub(super) async fn queue_completed(
    source: &mut dyn LibrarySource,
    net: &DelayedNet,
    query: &str,
    events: usize,
) {
    source.write("query", &WriteValue::Text(query.into()));
    time::sleep(Duration::from_millis(300)).await;
    source.tick();
    wait_until(
        Duration::from_secs(2),
        "the completed request awaits a tick",
        || net.events().len() == events,
    )
    .await
    .expect("the transport must finish before the source drains it");
}

/// Drops the source and waits until its tasks release the transport, so the
/// event log is final and the mock verifies on the test thread.
pub(super) async fn release(source: Box<dyn LibrarySource>, net: &DelayedNet) {
    drop(source);
    wait_until(
        Duration::from_secs(2),
        "transport clones are released",
        || Arc::strong_count(&net.events) == 1,
    )
    .await
    .expect("the source tasks must release the transport before mock verification");
}

/// The reaction state of every row of track `id`.
pub(super) fn reactions(source: &dyn LibrarySource, id: &str) -> Vec<bool> {
    source
        .rows(None)
        .iter()
        .flat_map(|row| row.cells().iter().filter(|cell| cell.action() == Some(id)))
        .map(|cell| {
            let TableValue::Icon { icon, active, .. } = cell.value() else {
                panic!("a reaction cell shows an icon");
            };
            assert_eq!(
                *icon,
                if *active {
                    IconName::HeartFilled
                } else {
                    IconName::Heart
                }
            );
            *active
        })
        .collect()
}

/// Records each POST's start and end, and holds a `blocked` search until
/// cancellation drops it.
#[derive(Clone)]
pub(super) struct DelayedNet {
    net: Unimock,
    events: Arc<Mutex<Vec<String>>>,
}

impl DelayedNet {
    pub(super) fn new(net: Unimock) -> Self {
        Self {
            net,
            events: Arc::default(),
        }
    }

    pub(super) fn events(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("request event recording must not panic")
            .clone()
    }
}

struct Active {
    events: Arc<Mutex<Vec<String>>>,
    query: String,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.events
            .lock()
            .expect("request completion recording must retain the event log")
            .push(format!("end:{}", self.query));
    }
}

#[async_trait]
impl Net for DelayedNet {
    async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> Result<Bytes, NetError> {
        self.net.get_bytes(url, headers).await
    }

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> Result<ByteStream, NetError> {
        self.net.get_range(url, range, headers).await
    }

    async fn head(&self, url: Url, headers: Option<Headers>) -> Result<Headers, NetError> {
        self.net.head(url, headers).await
    }

    async fn stream(&self, url: Url, headers: Option<Headers>) -> Result<ByteStream, NetError> {
        self.net.stream(url, headers).await
    }

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> Result<Bytes, NetError> {
        let value: Value =
            serde_json::from_slice(&body).expect("the client must encode a JSON GraphQL request");
        let query = value["variables"]["query"]
            .as_str()
            .unwrap_or("node")
            .to_owned();
        self.events
            .lock()
            .expect("request start recording must retain the event log")
            .push(format!("start:{query}"));
        let _active = Active {
            events: self.events.clone(),
            query: query.clone(),
        };
        if query == "blocked" {
            std::future::pending::<()>().await;
        }
        self.net.post_bytes(url, body, headers).await
    }
}
