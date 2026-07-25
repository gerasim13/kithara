#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

//! `FileSrc::Resource` — a remote file whose cache key belongs to the caller
//! rather than being derived from its URL. This is what lets an HLS segment be
//! an ordinary file: the key comes from the variant's scope, the bytes come
//! from the segment URL, and the fetch/commit path is kithara-file's.

use std::{
    io::Read,
    sync::atomic::{AtomicUsize, Ordering},
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use bytes::Bytes;
use kithara::{
    assets::{
        AssetResource, AssetResourceState, AssetSource, AssetStore, AssetStoreBuilder, ChunkSink,
        ProcessCtx, ResourceKey, ResourceProcessor, StorageBackend,
    },
    drm::{DecryptContext, as_process_ctx},
    file::{FetchCompleteFn, FetchOutcome, File, FileConfig, FileSrc},
    platform::{
        sync::{Arc, Mutex},
        time::Duration,
        tokio::task::spawn_blocking,
    },
    stream::Stream,
};
use kithara_integration_tests::{TestHttpServer, hls_fixture::crypto, kithara};
use num_traits::AsPrimitive;

/// Long enough that a cut at `CUT` leaves a real gap to resume.
const BODY_LEN: usize = 40_000;
/// Bytes the first response delivers before the body ends early.
const CUT: usize = 12_000;

fn body_bytes() -> Vec<u8> {
    (0..BODY_LEN)
        .map(|i| u8::try_from(i % 251).expect("modulo 251 fits u8"))
        .collect()
}

#[derive(Clone)]
struct ServeState {
    data: Arc<Vec<u8>>,
    gets: Arc<AtomicUsize>,
    /// Cut the first response short so the fetch has to resume.
    cut_first: bool,
}

fn range_start(headers: &HeaderMap) -> usize {
    headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split('-').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

async fn serve(State(state): State<ServeState>, headers: HeaderMap) -> Response {
    let nth = state.gets.fetch_add(1, Ordering::SeqCst);
    let total = state.data.len();
    let start = range_start(&headers);
    let tail = &state.data[start.min(total)..];

    // First response advertises the full remaining length but stops early, so
    // the engine sees a gap and issues a ranged follow-up.
    let (status, sent) = if state.cut_first && nth == 0 {
        (StatusCode::OK, &tail[..CUT.min(tail.len())])
    } else if start > 0 {
        (StatusCode::PARTIAL_CONTENT, tail)
    } else {
        (StatusCode::OK, tail)
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, tail.len().to_string())
        .header(header::CONTENT_TYPE, "audio/mpeg");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, total - 1, total),
        );
    }
    builder
        .body(Body::from(Bytes::copy_from_slice(sent)))
        .expect("valid response")
}

async fn start_server(data: Vec<u8>, cut_first: bool) -> (TestHttpServer, Arc<AtomicUsize>) {
    let gets = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/segment-0.ts", get(serve))
        .with_state(ServeState {
            data: Arc::new(data),
            gets: Arc::clone(&gets),
            cut_first,
        });
    (TestHttpServer::new(app).await, gets)
}

fn memory_store() -> AssetStore {
    AssetStoreBuilder::default()
        .backend(StorageBackend::Memory)
        .build()
}

/// A key minted the way an HLS variant mints one: from the master's scope,
/// with the segment URL as the resource — deliberately unrelated to the key
/// `FileSrc::Remote` would derive from the fetch URL.
fn caller_key(store: &AssetStore, segment: &url::Url) -> ResourceKey {
    store
        .scope::<File>(&AssetSource::Remote {
            url: url::Url::parse("https://example.com/master.m3u8").expect("master url"),
            discriminator: Some("caller-owned".to_string()),
        })
        .expect("caller scope")
        .key(&AssetResource::Url(segment.clone()))
        .expect("caller key")
}

/// The key `FileSrc::Remote` would have derived from the same URL — used to
/// prove no such key is minted behind the caller's back.
fn url_derived_key(store: &AssetStore, segment: &url::Url) -> ResourceKey {
    store
        .scope::<File>(&AssetSource::Remote {
            url: segment.clone(),
            discriminator: None,
        })
        .expect("url scope")
        .key(&AssetResource::Source {
            extension: "ts".to_string(),
        })
        .expect("url key")
}

/// `read_to_end` blocks; off the blocking pool it would pin the test's async
/// thread and the downloader would never be driven.
async fn read_all(mut stream: Stream<File>) -> Vec<u8> {
    spawn_blocking(move || {
        let mut out = Vec::new();
        stream.read_to_end(&mut out).expect("read to end");
        out
    })
    .await
    .expect("blocking read task")
}

async fn open(store: &AssetStore, key: &ResourceKey, url: &url::Url) -> Stream<File> {
    open_with(store, key, url, None).await
}

async fn open_with(
    store: &AssetStore,
    key: &ResourceKey,
    url: &url::Url,
    process: Option<ProcessCtx>,
) -> Stream<File> {
    open_full(store, key, url, process, None).await
}

async fn open_full(
    store: &AssetStore,
    key: &ResourceKey,
    url: &url::Url,
    process: Option<ProcessCtx>,
    on_fetch_complete: Option<FetchCompleteFn>,
) -> Stream<File> {
    let config = FileConfig::for_src(FileSrc::Resource {
        key: key.clone(),
        url: url.clone(),
    })
    .store(store.clone())
    .maybe_process(process)
    .maybe_on_fetch_complete(on_fetch_complete)
    .build();
    Stream::<File>::new(config)
        .await
        .expect("open caller-keyed file")
}

/// Collects what the fetch reported, so a test can assert on the outcome the
/// caller would drive its own state machine from.
#[derive(Clone, Default)]
struct Reported(Arc<Mutex<Vec<FetchOutcome>>>);

impl Reported {
    fn hook(&self) -> FetchCompleteFn {
        let sink = Arc::clone(&self.0);
        Arc::new(move |outcome| sink.lock().push(outcome))
    }

    fn outcomes(&self) -> Vec<FetchOutcome> {
        self.0.lock().clone()
    }
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn bytes_land_under_the_caller_key() {
    let data = body_bytes();
    let (server, gets) = start_server(data.clone(), false).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);

    assert_eq!(read_all(open(&store, &key, &url).await).await, data);

    let body_len: u64 = data.len().as_();
    assert_eq!(gets.load(Ordering::SeqCst), 1, "one GET for one fetch");
    assert!(
        matches!(
            store.resource_state(&key).expect("caller key state"),
            AssetResourceState::Committed { final_len: Some(len) } if len == body_len
        ),
        "the caller's key holds the committed bytes"
    );
    assert_eq!(
        store
            .resource_state(&url_derived_key(&store, &url))
            .expect("url key state"),
        AssetResourceState::Missing,
        "no key is minted from the fetch URL"
    );
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn committed_caller_key_skips_the_download() {
    let data = body_bytes();
    let (server, gets) = start_server(data.clone(), false).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);

    assert_eq!(read_all(open(&store, &key, &url).await).await, data);
    assert_eq!(gets.load(Ordering::SeqCst), 1);

    assert_eq!(read_all(open(&store, &key, &url).await).await, data);
    assert_eq!(
        gets.load(Ordering::SeqCst),
        1,
        "the cached-hit fast path must not re-fetch a committed caller key"
    );
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn caller_key_resumes_from_the_reached_byte() {
    let data = body_bytes();
    let (server, gets) = start_server(data.clone(), true).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);

    assert_eq!(
        read_all(open(&store, &key, &url).await).await,
        data,
        "an interrupted fetch completes on the caller's key"
    );
    assert!(
        gets.load(Ordering::SeqCst) >= 2,
        "the cut body forces at least one ranged follow-up"
    );
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn a_finished_fetch_reports_the_committed_length() {
    let data = body_bytes();
    let (server, _gets) = start_server(data.clone(), false).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);
    let reported = Reported::default();

    let read = read_all(open_full(&store, &key, &url, None, Some(reported.hook())).await).await;

    let body_len: u64 = data.len().as_();
    assert_eq!(read, data);
    assert!(
        matches!(
            reported.outcomes().as_slice(),
            [FetchOutcome::Committed { final_len: Some(len) }] if *len == body_len
        ),
        "one fetch reports one outcome, carrying the length the caller needs to size its own \
         layout, got {:?}",
        reported.outcomes()
    );
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn a_cache_hit_still_reports_a_commit() {
    let data = body_bytes();
    let (server, gets) = start_server(data.clone(), false).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);
    read_all(open(&store, &key, &url).await).await;

    let reported = Reported::default();
    read_all(open_full(&store, &key, &url, None, Some(reported.hook())).await).await;

    let body_len: u64 = data.len().as_();
    assert_eq!(
        gets.load(Ordering::SeqCst),
        1,
        "the second open is a cache hit"
    );
    assert!(
        matches!(
            reported.outcomes().as_slice(),
            [FetchOutcome::Committed { final_len: Some(len) }] if *len == body_len
        ),
        "a fetch that never had to run still ended — a caller waiting on the outcome must not \
         wait forever, got {:?}",
        reported.outcomes()
    );
}

/// Drops a fixed trailer on the last chunk, so the committed length is shorter
/// than the fetched length — the same shape PKCS7 unpadding produces.
#[derive(Debug)]
struct TrailerStripper;

const TRAILER: usize = 4;

impl ResourceProcessor for TrailerStripper {
    fn begin(&self) -> Box<dyn ChunkSink> {
        Box::new(TrailerStripper)
    }

    fn identity(&self) -> &[u8] {
        b"trailer-stripper"
    }
}

impl ChunkSink for TrailerStripper {
    fn process(&mut self, input: &[u8], output: &mut [u8], is_last: bool) -> Result<usize, String> {
        let keep = if is_last {
            input.len().saturating_sub(TRAILER)
        } else {
            input.len()
        };
        output[..keep].copy_from_slice(&input[..keep]);
        Ok(keep)
    }
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn processing_context_transforms_on_commit() {
    let data = body_bytes();
    let (server, _gets) = start_server(data.clone(), false).await;
    let url = server.url("/segment-0.ts");
    let store = memory_store();
    let key = caller_key(&store, &url);
    let process: ProcessCtx = Arc::new(TrailerStripper);

    let read = read_all(open_with(&store, &key, &url, Some(process)).await).await;

    let processed_len: u64 = (data.len() - TRAILER).as_();
    assert_eq!(
        read,
        data[..data.len() - TRAILER],
        "the reader sees the processed bytes"
    );
    assert!(
        matches!(
            store.resource_state(&key).expect("state"),
            AssetResourceState::Committed { final_len: Some(len) } if len == processed_len
        ),
        "the committed length is the processed length, read back after commit"
    );
}

/// The resume × CBC pin: `run_process` decrypts at commit over the complete
/// on-disk ciphertext, so it cannot tell a resumed fetch from a single-shot
/// one. Interrupt an encrypted fetch and require byte-identical plaintext.
#[kithara::test(
    tokio,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "5")
)]
async fn encrypted_caller_key_resumes_to_identical_plaintext() {
    let plaintext = body_bytes();
    let ciphertext = crypto::aes128_encrypt(&plaintext);
    let decrypt = || {
        let key: [u8; 16] = crypto::aes128_key_bytes()[..16]
            .try_into()
            .expect("aes key len");
        as_process_ctx(DecryptContext::new(key, crypto::aes128_iv()))
    };

    let (whole_server, _) = start_server(ciphertext.clone(), false).await;
    let whole_url = whole_server.url("/segment-0.ts");
    let whole_store = memory_store();
    let whole_key = caller_key(&whole_store, &whole_url);
    let uninterrupted =
        read_all(open_with(&whole_store, &whole_key, &whole_url, Some(decrypt())).await).await;

    let (cut_server, cut_gets) = start_server(ciphertext, true).await;
    let cut_url = cut_server.url("/segment-0.ts");
    let cut_store = memory_store();
    let cut_key = caller_key(&cut_store, &cut_url);
    let resumed = read_all(open_with(&cut_store, &cut_key, &cut_url, Some(decrypt())).await).await;

    assert_eq!(uninterrupted, plaintext, "control: plaintext round-trips");
    assert!(
        cut_gets.load(Ordering::SeqCst) >= 2,
        "the encrypted fetch actually resumed"
    );
    assert_eq!(
        resumed, plaintext,
        "a resumed ciphertext decrypts identically — the CBC chain is rebuilt at commit"
    );
}
