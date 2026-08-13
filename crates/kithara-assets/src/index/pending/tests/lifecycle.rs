use super::*;

#[kithara::test(timeout(Duration::from_secs(1)))]
fn attach_refcount_and_single_writer_election() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");

    let attach_count = AtomicUsize::new(0);
    let writer_wins = AtomicUsize::new(0);

    let mut leases = Vec::new();
    // Hold every elected handle: a dropped `WriterHandle` re-opens
    // the election, so the single-winner invariant only holds while
    // the elected writer stays alive.
    let mut writers = Vec::new();
    for _ in 0..8 {
        let (lease, writer) = attach(&index, &store, &key, entry(0, Some(64)));
        attach_count.fetch_add(1, Ordering::Relaxed);
        if let Some(handle) = writer {
            writer_wins.fetch_add(1, Ordering::Relaxed);
            writers.push(handle);
        }
        leases.push(lease);
    }

    assert_eq!(attach_count.load(Ordering::Relaxed), 8);
    assert_eq!(
        writer_wins.load(Ordering::Relaxed),
        1,
        "exactly one attacher wins the writer election"
    );
    drop(writers);

    // Last detach cancels the writer and removes the slot.
    let writer_cancel = leases[0].session_cancel();
    assert!(!writer_cancel.is_cancelled());
    drop(leases);
    assert!(
        writer_cancel.is_cancelled(),
        "dropping the last lease cancels writer_cancel"
    );
    assert!(
        !index.has_slot_for_test(&key),
        "dropping the last lease removes the slot"
    );
}

#[kithara::test]
fn cancelled_session_does_not_reopen_writer_election() {
    let scope = CancelScope::new(None);
    let index = PendingResourceIndex::new(scope.token());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");
    let (lease, writer) = attach(&index, &store, &key, entry(0, Some(64)));
    drop(writer);

    scope.cancel();

    assert!(lease.try_take_writer().is_none());
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn watermark_is_max_over_entries() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");

    // Bounded entry: read_pos 10 + look_ahead 50 = 60.
    let (_l1, writer) = attach(&index, &store, &key, entry(10, Some(50)));
    let writer = writer.expect("first attach wins the writer");
    assert_eq!(writer.max_watermark(), 60);

    // Unbounded entry collapses the aggregate to u64::MAX.
    let (_l2, none) = attach(&index, &store, &key, entry(0, None));
    assert!(none.is_none(), "second attach is not the writer");
    assert_eq!(writer.max_watermark(), u64::MAX);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn watermark_tracks_read_pos_advance() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");
    let read_pos = Arc::new(AtomicU64::new(0));

    let (lease, writer) = attach(
        &index,
        &store,
        &key,
        Arc::new(DemandEntry::new(Arc::clone(&read_pos), Some(100))),
    );
    let writer = writer.expect("first attach wins the writer");
    assert_eq!(writer.max_watermark(), 100);

    read_pos.store(500, Ordering::Release);
    lease.note_progress();
    assert_eq!(writer.max_watermark(), 600);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn immediate_request_extends_bounded_watermark() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");
    let (lease, writer) = attach(&index, &store, &key, entry(0, Some(0)));
    let writer = writer.expect("first attach wins the writer");
    assert_eq!(writer.max_watermark(), 0);

    lease.request_until(32);

    assert_eq!(writer.max_watermark(), 32);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn detach_one_of_two_keeps_slot_and_writer() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");

    let (l1, writer) = attach(&index, &store, &key, entry(0, Some(10)));
    let writer = writer.expect("first attach wins");
    let (l2, _none) = attach(&index, &store, &key, entry(0, Some(10)));

    drop(l1);
    assert!(
        !writer.writer_cancel().is_cancelled(),
        "writer survives while one consumer remains"
    );
    assert!(index.has_slot_for_test(&key));

    drop(l2);
    assert!(writer.writer_cancel().is_cancelled());
    assert!(!index.has_slot_for_test(&key));
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn reattach_after_last_detach_wins_writer_election() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");

    let (l1, _writer) = attach(&index, &store, &key, entry(0, None));
    // Probe: slot is live, second attach must not win the election.
    let (l2, probe) = attach(&index, &store, &key, entry(0, None));
    assert!(
        probe.is_none(),
        "second attach while slot is live is not a writer"
    );

    drop(l1);
    drop(l2);
    assert!(
        !index.has_slot_for_test(&key),
        "slot removed after last detach"
    );

    // Fresh attach must win the writer election on the cleared slot.
    let (_l3, new_writer) = attach(&index, &store, &key, entry(0, None));
    assert!(
        new_writer.is_some(),
        "reattach after slot removal wins the writer election"
    );
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn dropping_writer_lets_a_survivor_take_over() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "file");

    let (winner_lease, writer) = attach(&index, &store, &key, entry(0, None));
    let writer = writer.expect("first attach wins the writer");
    let (survivor_lease, none) = attach(&index, &store, &key, entry(0, None));
    assert!(none.is_none(), "second attach is not the writer");

    // While the writer is alive the survivor cannot take over.
    assert!(
        survivor_lease.try_take_writer().is_none(),
        "election stays closed while the writer is alive"
    );

    drop(writer);
    drop(winner_lease);
    assert!(
        index.has_slot_for_test(&key),
        "slot survives while one consumer remains"
    );

    let taken = survivor_lease
        .try_take_writer()
        .expect("survivor takes over the abandoned slot");
    assert_eq!(taken.max_watermark(), u64::MAX);
    assert!(
        survivor_lease.try_take_writer().is_none(),
        "only one survivor takes over"
    );
}
