use super::*;

#[kithara::test(timeout(Duration::from_secs(1)))]
fn write_and_terminal_wake_every_attached_reader_and_peer() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "reader-wake");
    let (first_lease, writer) = attach(&index, &store, &key, entry(0, None));
    let (follower_lease, none) = attach(&index, &store, &key, entry(0, None));
    assert!(none.is_none());
    let writer = writer.expect("first writer");
    let writer_cancel = writer.writer_cancel();
    let epoch = writer.epoch();
    let (first_reader_count, first_reader_waker) = counting_waker();
    let (follower_reader_count, follower_reader_waker) = counting_waker();
    let (first_peer_count, first_peer_waker) = counting_waker();
    let (follower_peer_count, follower_peer_waker) = counting_waker();
    first_lease.register_reader_waker(&first_reader_waker);
    follower_lease.register_reader_waker(&follower_reader_waker);
    first_lease.register_peer_waker(&first_peer_waker);
    follower_lease.register_peer_waker(&follower_peer_waker);

    epoch
        .write_at(0, b"wake")
        .current()
        .expect("current write")
        .expect("write succeeds");
    assert_eq!(first_reader_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(follower_reader_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(first_peer_count.0.load(Ordering::SeqCst), 0);
    assert_eq!(follower_peer_count.0.load(Ordering::SeqCst), 0);

    epoch
        .write_at(0, b"wake")
        .current()
        .expect("current repeat write")
        .expect("repeat write succeeds");
    assert_eq!(first_reader_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(follower_reader_count.0.load(Ordering::SeqCst), 1);

    first_lease.register_reader_waker(&first_reader_waker);
    follower_lease.register_reader_waker(&follower_reader_waker);
    epoch
        .commit(Some(4))
        .current()
        .expect("current commit")
        .expect("commit succeeds");
    assert_eq!(first_reader_count.0.load(Ordering::SeqCst), 2);
    assert_eq!(follower_reader_count.0.load(Ordering::SeqCst), 2);
    assert_eq!(first_peer_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(follower_peer_count.0.load(Ordering::SeqCst), 1);
    assert!(writer_cancel.is_cancelled());
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn follower_attach_and_progress_wake_the_current_writer_peer() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "writer-wake");
    let (writer_lease, writer) = attach(&index, &store, &key, entry(0, None));
    let (count, waker) = counting_waker();
    writer_lease.register_peer_waker(&waker);
    writer_lease.register_peer_waker(&waker);

    let (follower_lease, none) = attach(&index, &store, &key, entry(0, None));
    assert!(none.is_none());
    assert_eq!(count.0.load(Ordering::SeqCst), 1);

    follower_lease.note_progress();
    assert_eq!(count.0.load(Ordering::SeqCst), 1);

    writer_lease.register_peer_waker(&waker);
    follower_lease.note_progress();
    assert_eq!(count.0.load(Ordering::SeqCst), 2);
    drop(writer);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn writer_drop_wakes_a_registered_follower_peer_once() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "handoff-wake");
    let (_writer_lease, writer) = attach(&index, &store, &key, entry(0, None));
    let (follower_lease, none) = attach(&index, &store, &key, entry(0, None));
    assert!(none.is_none());
    let (count, waker) = counting_waker();
    follower_lease.register_peer_waker(&waker);
    follower_lease.register_peer_waker(&waker);

    drop(writer.expect("first writer"));

    assert_eq!(count.0.load(Ordering::SeqCst), 1);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn cleared_peer_registration_is_not_woken_by_progress() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "clear-peer-wake");
    let (lease, writer) = attach(&index, &store, &key, entry(0, None));
    let _writer = writer.expect("first writer");
    let (count, waker) = counting_waker();
    lease.register_peer_waker(&waker);

    lease.clear_peer_waker(&waker);
    lease.note_progress();

    assert_eq!(count.0.load(Ordering::SeqCst), 0);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn clearing_stale_peer_waker_preserves_its_replacement() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "replace-peer-wake");
    let (lease, writer) = attach(&index, &store, &key, entry(0, None));
    let _writer = writer.expect("first writer");
    let (old_count, old_waker) = counting_waker();
    let (new_count, new_waker) = counting_waker();
    lease.register_peer_waker(&old_waker);
    lease.register_peer_waker(&new_waker);

    lease.clear_peer_waker(&old_waker);
    lease.note_progress();

    assert_eq!(old_count.0.load(Ordering::SeqCst), 0);
    assert_eq!(new_count.0.load(Ordering::SeqCst), 1);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn waker_replacement_drops_old_after_slot_unlock() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "reentrant-waker-drop");
    let (lease, writer) = attach(&index, &store, &key, entry(0, None));
    let lease = Arc::new(lease);
    let writer = writer.expect("first writer");
    let epoch = writer.epoch();
    let (discarded_count, discarded_waker) = counting_waker();
    let (replacement_count, replacement_waker) = counting_waker();
    let dropped = Arc::new(AtomicBool::new(false));
    let reentrant = Waker::from(Arc::new(RearmReaderOnDrop {
        dropped: Arc::clone(&dropped),
        lease: Arc::clone(&lease),
        replacement: replacement_waker,
    }));
    lease.register_reader_waker(&reentrant);
    drop(reentrant);

    lease.register_reader_waker(&discarded_waker);

    assert!(dropped.load(Ordering::SeqCst));
    epoch
        .write_at(0, b"wake")
        .current()
        .expect("current write")
        .expect("write succeeds");
    assert_eq!(discarded_count.0.load(Ordering::SeqCst), 0);
    assert_eq!(replacement_count.0.load(Ordering::SeqCst), 1);
}
