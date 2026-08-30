#[cfg(not(target_arch = "wasm32"))]
use std::{sync::Barrier, thread};
use std::{
    sync::atomic::AtomicU64,
    task::{Wake, Waker},
};

use kithara_assets::{
    AcquisitionResult, AssetReader, AssetResource, AssetResourceState, AssetSource, AssetStore,
    ResourceKey, ResourceLease, StorageBackend, WriterHandle, WriterOutcome,
};
use kithara_events::{Envelope, Event, EventBus, FileEvent};
use kithara_platform::{CancelScope, CancelToken, sync::Arc, time::Duration};
use kithara_stream::{PlayheadState, SeekState, WorkerWake, dl::Peer};
use kithara_test_utils::kithara;
use url::Url;

use super::*;
use crate::{
    File,
    coord::FileCoord,
    session::{FileSource, inner::FileSourceCtx},
    test_pools::{TestPools, pools},
};

type TestFile = File<TestPools>;
type TestInner = FileInner<TestPools>;
type TestLease = ResourceLease<TestPools>;
type TestPeer = FilePeer<TestPools>;
type TestReader = AssetReader<TestPools>;
type TestStore = AssetStore<TestPools>;
type TestWriterHandle = WriterHandle<TestPools>;

mod completion;
mod metadata;
mod ownership;
mod seek;

fn test_key(store: &TestStore) -> ResourceKey {
    let source = AssetSource::Remote {
        url: Url::parse("https://example.com/remote.dat").expect("test URL"),
        discriminator: Some("peer-test".to_string()),
    };
    let scope = store.scope::<TestFile>(&source).expect("test scope");
    scope
        .key(&AssetResource::Source {
            extension: "dat".to_string(),
        })
        .expect("test resource key")
}

fn make_coord() -> Arc<FileCoord> {
    Arc::new(FileCoord::new(
        Arc::new(PlayheadState::new()),
        Arc::new(SeekState::new()),
    ))
}

fn attach_pending(
    store: &TestStore,
    key: &ResourceKey,
    coord: &Arc<FileCoord>,
    look_ahead: Option<u64>,
) -> (TestReader, TestLease, Option<TestWriterHandle>) {
    let AcquisitionResult::Pending(attachment) = store
        .attach_pending_resource(key, coord.read_pos_handle(), look_ahead)
        .expect("attach pending resource")
    else {
        panic!("fresh session must be pending");
    };
    attachment.into()
}

fn make_inner(
    reader: TestReader,
    lease: TestLease,
    coord: Arc<FileCoord>,
    bus: EventBus,
) -> Arc<TestInner> {
    make_inner_with_cancel(reader, lease, coord, bus, CancelToken::never())
}

fn make_inner_with_cancel(
    reader: TestReader,
    lease: TestLease,
    coord: Arc<FileCoord>,
    bus: EventBus,
    cancel: CancelToken,
) -> Arc<TestInner> {
    Arc::new(FileInner::new(
        FileSourceCtx {
            coord,
            cancel,
            bus,
            reader_event_capacity: 16,
        },
        crate::session::inner::FileAssetCtx {
            reader,
            headers: None,
            url: Url::parse("http://127.0.0.1/test.mp3").expect("test url"),
        },
        false,
        Some(lease),
    ))
}

fn make_peer(inner: &Arc<TestInner>, writer: Option<TestWriterHandle>) -> TestPeer {
    FilePeer::new(inner, writer)
}

fn completion(
    resume_from: u64,
    bytes_written: u64,
    end_exclusive: Option<u64>,
    error: Option<&NetError>,
) -> FetchCompletion<'_> {
    FetchCompletion {
        bytes_written,
        end_exclusive,
        error,
        resume_from,
        invalid_response: false,
    }
}

#[derive(Default)]
struct CountingWake(AtomicU64);

#[cfg(not(target_arch = "wasm32"))]
struct BlockingWake {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl CountingWake {
    fn count(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

impl WorkerWake for CountingWake {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    fn defer(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl WorkerWake for BlockingWake {
    fn wake(&self) {
        self.entered.wait();
        self.release.wait();
    }

    fn defer(&self) {}
}

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

fn fresh_session(
    look_ahead: Option<u64>,
) -> (TestStore, ResourceKey, Arc<TestInner>, TestWriterHandle) {
    let store = AssetStore::builder(pools())
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = test_key(&store);
    let coord = make_coord();
    let (reader, lease, writer) = attach_pending(&store, &key, &coord, look_ahead);
    let writer = writer.expect("first consumer is writer");
    let inner = make_inner(reader, lease, coord, EventBus::new(16));
    (store, key, inner, writer)
}

fn assert_ready_bytes(store: &TestStore, key: &ResourceKey, expected: &[u8]) {
    let AcquisitionResult::Ready(reader) = store
        .attach_pending_resource(key, Arc::new(AtomicU64::new(0)), None)
        .expect("reopen committed session")
    else {
        panic!("committed session must reopen ready");
    };
    let mut bytes = vec![0; expected.len()];
    let read = reader.read_at(0, &mut bytes).expect("read committed bytes");
    assert_eq!(read, expected.len());
    assert_eq!(bytes, expected);
}
