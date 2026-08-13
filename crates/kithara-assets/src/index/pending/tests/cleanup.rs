use super::*;

#[kithara::test(timeout(Duration::from_secs(1)))]
fn cleanup_failure_blocks_successor_publication() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "cleanup-error");
    let remove_calls = Arc::new(AtomicUsize::new(0));
    let counted_remove = Arc::clone(&remove_calls);
    let remove: RemoveResource = Arc::new(move |_| {
        counted_remove.fetch_add(1, Ordering::SeqCst);
        Err(AssetsError::InvalidKey)
    });
    let acquisition = index
        .attach_pending_resource(
            &key,
            entry(0, None),
            store.clone(),
            Arc::clone(&remove),
            || store.acquire_resource(&key, None),
        )
        .expect("initial attachment");
    let AcquisitionResult::Pending(attachment) = acquisition else {
        panic!("fresh resource must start an acquisition session");
    };
    let (reader, lease, writer) = attachment.into();
    let writer = writer.expect("initial attachment owns the writer");
    writer
        .epoch()
        .write_at(0, b"live")
        .current()
        .expect("writer epoch remains current")
        .expect("writer write");
    assert!(store.contains_range(&key, 0..4));
    drop(writer);
    drop(reader);
    drop(lease);

    assert!(
        index.has_slot_for_test(&key),
        "failed cleanup must keep an exact tombstone"
    );
    assert_eq!(
        remove_calls.load(Ordering::SeqCst),
        1,
        "the acquisition session is the sole physical cleanup owner"
    );
    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Active
    ));
    assert!(
        store.contains_range(&key, 0..4),
        "failed canonical cleanup must not hide an earlier physical remove"
    );
    let successor =
        index.attach_pending_resource(&key, entry(0, None), store.clone(), remove, || {
            store.acquire_resource(&key, None)
        });
    let Err(AssetsError::Storage(StorageError::Io(error))) = successor else {
        panic!("successor must receive typed cleanup failure");
    };
    let cleanup = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<PendingResourceCleanupError>())
        .expect("io error retains typed cleanup carrier");
    assert_eq!(cleanup.key(), &key);
    assert!(cleanup.to_string().contains("cleanup-error"));
    let source = StdError::source(cleanup).expect("cleanup source");
    assert!(matches!(
        source.downcast_ref::<AssetsError>(),
        Some(AssetsError::InvalidKey)
    ));
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PendingResourceCleanupError>();
}
