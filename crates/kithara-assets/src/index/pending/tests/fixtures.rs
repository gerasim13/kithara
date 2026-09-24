use std::{
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    task::{Wake, Waker},
};

use kithara_platform::{CancelToken, sync::Arc};

use super::super::*;
use crate::{AcquisitionResult, AssetStore, StorageBackend, WriterHandle, layout::ResourceKey};

type TestAssetStore = AssetStore<crate::test_pools::TestPools>;
type TestPendingResourceIndex = PendingResourceIndex<crate::test_pools::TestPools>;
type TestResourceLease = ResourceLease<crate::test_pools::TestPools>;
type TestWriterHandle = WriterHandle<crate::test_pools::TestPools>;

#[derive(Default)]
pub(super) struct WakeCount(pub(super) AtomicUsize);

pub(super) struct RearmReaderOnDrop {
    pub(super) dropped: Arc<AtomicBool>,
    pub(super) lease: Arc<TestResourceLease>,
    pub(super) replacement: Waker,
}

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Wake for RearmReaderOnDrop {
    fn wake(self: Arc<Self>) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl Drop for RearmReaderOnDrop {
    fn drop(&mut self) {
        self.lease.register_reader_waker(&self.replacement);
        self.dropped.store(true, Ordering::SeqCst);
    }
}

pub(super) fn counting_waker() -> (Arc<WakeCount>, Waker) {
    let count = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&count));
    (count, waker)
}

pub(super) fn entry(read_pos: u64, look_ahead: Option<u64>) -> Arc<DemandEntry> {
    Arc::new(DemandEntry::new(
        Arc::new(AtomicU64::new(read_pos)),
        look_ahead,
    ))
}

pub(super) fn test_store() -> TestAssetStore {
    AssetStore::builder(crate::test_pools::pools())
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build()
}

pub(super) fn attach(
    index: &TestPendingResourceIndex,
    store: &TestAssetStore,
    key: &ResourceKey,
    entry: Arc<DemandEntry>,
) -> (TestResourceLease, Option<TestWriterHandle>) {
    let remove_store = store.clone();
    let remove: RemoveResource = Arc::new(move |key| remove_store.remove_resource(key));
    let acquisition = index
        .attach_pending_resource(key, entry, store.clone(), remove, || {
            store.acquire_resource(key, None)
        })
        .expect("test attachment");
    let AcquisitionResult::Pending(attachment) = acquisition else {
        panic!("test resource must be active");
    };
    let (_reader, lease, writer) = attachment.into();
    (lease, writer)
}
