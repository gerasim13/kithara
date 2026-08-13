use super::*;

#[kithara::test]
fn cancelled_waiting_source_relinquishes_writer() {
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = test_key(&store);
    let first_coord = make_coord();
    let (first_reader, first_lease, first_writer) =
        attach_pending(&store, &key, &first_coord, Some(4));
    let scope = CancelScope::new(None);
    let inner = make_inner_with_cancel(
        first_reader,
        first_lease,
        first_coord,
        EventBus::new(16),
        scope.token(),
    );
    let first_writer = first_writer.expect("first consumer is writer");
    assert!(matches!(
        first_writer.epoch().write_at(0, b"data"),
        WriterOutcome::Current(Ok(()))
    ));
    let peer = make_peer(&inner, Some(first_writer));

    let follower_coord = make_coord();
    let (_reader, follower_lease, follower_writer) =
        attach_pending(&store, &key, &follower_coord, Some(4));
    assert!(follower_writer.is_none());
    let wake = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));

    scope.cancel();
    assert_eq!(wake.count(), 1, "source cancellation must wake its peer");

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Ready(None)));
    assert!(peer.writer.lock().is_none());
    assert!(follower_lease.try_take_writer().is_some());
}

#[kithara::test]
fn cancelled_download_session_wakes_peer_and_prevents_reelection() {
    let store_scope = CancelScope::new(None);
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(store_scope.token())
        .build();
    let key = test_key(&store);
    let first_coord = make_coord();
    let (first_reader, first_lease, first_writer) =
        attach_pending(&store, &key, &first_coord, Some(4));
    let inner = make_inner(first_reader, first_lease, first_coord, EventBus::new(16));
    let first_writer = first_writer.expect("first consumer is writer");
    assert!(matches!(
        first_writer.epoch().write_at(0, b"data"),
        WriterOutcome::Current(Ok(()))
    ));
    let peer = make_peer(&inner, Some(first_writer));

    let follower_coord = make_coord();
    let (_reader, follower_lease, follower_writer) =
        attach_pending(&store, &key, &follower_coord, Some(4));
    assert!(follower_writer.is_none());
    let wake = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));

    store_scope.cancel();
    assert_eq!(wake.count(), 1, "session cancellation must wake its peer");

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Ready(None)));
    assert!(peer.writer.lock().is_none());
    assert!(follower_lease.try_take_writer().is_none());
}

#[kithara::test]
fn cancelled_active_session_does_not_start_another_fetch() {
    let store_scope = CancelScope::new(None);
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(store_scope.token())
        .build();
    let key = test_key(&store);
    let first_coord = make_coord();
    let (first_reader, first_lease, first_writer) =
        attach_pending(&store, &key, &first_coord, Some(4));
    let inner = make_inner(first_reader, first_lease, first_coord, EventBus::new(16));
    let first_writer = first_writer.expect("first consumer is writer");
    let epoch = first_writer.epoch();
    let peer = make_peer(&inner, Some(first_writer));

    let follower_coord = make_coord();
    let (_reader, follower_lease, follower_writer) =
        attach_pending(&store, &key, &follower_coord, Some(4));
    assert!(follower_writer.is_none());
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("missing bytes must start the first fetch");
    };
    drop(fetches);

    store_scope.cancel();
    inner.complete_fetch(
        &epoch,
        completion(0, 0, Some(4), Some(&NetError::Cancelled)),
    );
    peer.inflight.store(false, Ordering::Release);

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Ready(None)));
    assert!(peer.writer.lock().is_none());
    assert!(follower_lease.try_take_writer().is_none());
}

#[kithara::test]
fn writer_drop_promotes_follower_with_same_partial_bytes() {
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = test_key(&store);
    let first_coord = make_coord();
    let (first_reader, first_lease, first_writer) =
        attach_pending(&store, &key, &first_coord, None);
    let first_writer = first_writer.expect("first writer");
    let first_epoch = first_writer.epoch();
    assert!(matches!(
        first_epoch.write_at(0, b"old"),
        WriterOutcome::Current(Ok(()))
    ));
    let _first_inner = make_inner(first_reader, first_lease, first_coord, EventBus::new(16));

    let follower_coord = make_coord();
    let (follower_reader, follower_lease, follower_writer) =
        attach_pending(&store, &key, &follower_coord, None);
    assert!(follower_writer.is_none());
    let follower = make_inner(
        follower_reader,
        follower_lease,
        Arc::clone(&follower_coord),
        EventBus::new(16),
    );

    drop(first_writer);
    let promoted = follower
        .resource_lease
        .as_ref()
        .and_then(ResourceLease::try_take_writer)
        .expect("follower promotion");
    let promoted_epoch = promoted.epoch();
    assert!(matches!(
        first_epoch.write_at(0, b"bad"),
        WriterOutcome::Stale
    ));
    assert!(matches!(
        promoted_epoch.write_at(3, b"new"),
        WriterOutcome::Current(Ok(()))
    ));
    follower_coord.set_total_bytes(Some(6));
    follower.finalize_fetch(&promoted_epoch, completion(3, 3, None, None));

    assert_ready_bytes(&store, &key, b"oldnew");
    drop(promoted);
}

#[kithara::test]
fn late_cancelled_epoch_cannot_poison_successor() {
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = test_key(&store);
    let first_coord = make_coord();
    let (first_reader, first_lease, first_writer) =
        attach_pending(&store, &key, &first_coord, None);
    let first_writer = first_writer.expect("first writer");
    let first_epoch = first_writer.epoch();
    assert!(matches!(
        first_epoch.write_at(0, b"old"),
        WriterOutcome::Current(Ok(()))
    ));
    let first_bus = EventBus::new(16);
    let mut first_events = first_bus.subscribe();
    let first = make_inner(first_reader, first_lease, first_coord, first_bus);

    let successor_coord = make_coord();
    let (successor_reader, successor_lease, successor_writer) =
        attach_pending(&store, &key, &successor_coord, None);
    assert!(successor_writer.is_none());
    let successor_bus = EventBus::new(16);
    let mut successor_events = successor_bus.subscribe();
    let successor = make_inner(
        successor_reader,
        successor_lease,
        Arc::clone(&successor_coord),
        successor_bus,
    );

    first.finalize_fetch(
        &first_epoch,
        completion(0, 3, None, Some(&NetError::Cancelled)),
    );
    assert!(!first_writer.is_current());
    let successor_writer = successor
        .resource_lease
        .as_ref()
        .and_then(ResourceLease::try_take_writer)
        .expect("successor promotion");
    let successor_epoch = successor_writer.epoch();

    first.finalize_fetch(
        &first_epoch,
        completion(0, 3, None, Some(&NetError::Cancelled)),
    );
    assert!(first_events.try_recv().is_err());
    assert!(matches!(
        successor_epoch.write_at(3, b"new"),
        WriterOutcome::Current(Ok(()))
    ));
    successor_coord.set_total_bytes(Some(6));
    successor.finalize_fetch(&successor_epoch, completion(3, 3, None, None));
    assert_ready_bytes(&store, &key, b"oldnew");

    while let Ok(event) = successor_events.try_recv() {
        assert!(!matches!(event.event, Event::File(FileEvent::Error { .. })));
    }
    drop(successor_writer);
    drop(first_writer);
}

#[kithara::test]
fn dropping_last_file_source_clears_active_session_synchronously() {
    let store = AssetStore::builder()
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = test_key(&store);
    let coord = make_coord();
    let (reader, lease, writer) = attach_pending(&store, &key, &coord, None);
    let inner = make_inner(reader, lease, Arc::clone(&coord), EventBus::new(16));
    let peer = make_peer(&inner, writer);
    inner.arm_reader_waker();
    let source = FileSource::with_inner(Arc::clone(&inner), coord);

    drop(inner);
    drop(source);

    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Missing
    ));
    assert!(peer.inner.upgrade().is_none());
    let successor_coord = make_coord();
    assert!(matches!(
        store.attach_pending_resource(&key, successor_coord.read_pos_handle(), None),
        Ok(AcquisitionResult::Pending(_))
    ));
}
