use super::*;

#[kithara::test(timeout(Duration::from_secs(1)))]
fn follower_uses_the_elected_writers_session() {
    let store = AssetStore::builder(crate::test_pools::pools())
        .backend(StorageBackend::Memory)
        .cancel(CancelToken::never())
        .build();
    let key = ResourceKey::relative("asset", "file");

    let AcquisitionResult::Pending(first) = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), None)
        .expect("first attachment")
    else {
        panic!("fresh resource must start an acquisition session");
    };
    let (first_reader, _first_lease, writer) = first.into();
    let writer = writer.expect("first consumer owns the writer epoch");

    let AcquisitionResult::Pending(second) = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), Some(64))
        .expect("follower attachment")
    else {
        panic!("follower must join the active acquisition session");
    };
    let (second_reader, _second_lease, follower_writer) = second.into();
    assert!(follower_writer.is_none(), "follower must not own a writer");

    let epoch = writer.epoch();
    epoch
        .write_at(0, b"shared")
        .current()
        .expect("writer epoch remains current")
        .expect("writer write");
    epoch
        .commit(Some(6))
        .current()
        .expect("writer epoch remains current")
        .expect("writer commit");

    let mut first_bytes = [0; 6];
    let mut second_bytes = [0; 6];
    first_reader
        .read_at(0, &mut first_bytes)
        .expect("first reader");
    second_reader
        .read_at(0, &mut second_bytes)
        .expect("second reader");
    assert_eq!(&first_bytes, b"shared");
    assert_eq!(&second_bytes, b"shared");
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn committed_session_reopens_ready_after_consumers_detach() {
    let store = test_store();
    let key = ResourceKey::relative("asset", "committed");
    let AcquisitionResult::Pending(attachment) = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), None)
        .expect("initial attachment")
    else {
        panic!("fresh resource must start an acquisition session");
    };
    let (reader, lease, writer) = attachment.into();
    let writer = writer.expect("first consumer owns the writer epoch");
    let epoch = writer.epoch();
    epoch
        .write_at(0, b"ready")
        .current()
        .expect("writer epoch remains current")
        .expect("writer write");
    epoch
        .commit(Some(5))
        .current()
        .expect("writer epoch remains current")
        .expect("writer commit");
    drop(writer);
    drop(reader);
    drop(lease);

    let reopened = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), None)
        .expect("reopen committed resource");
    let AcquisitionResult::Ready(reader) = reopened else {
        panic!("committed session must reopen Ready");
    };
    let mut bytes = [0; 5];
    reader.read_at(0, &mut bytes).expect("committed read");
    assert_eq!(&bytes, b"ready");
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn writer_handoff_keeps_writer_and_rejects_old_epoch() {
    let store = test_store();
    let key = ResourceKey::relative("asset", "handoff");
    let AcquisitionResult::Pending(first) = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), None)
        .expect("first attachment")
    else {
        panic!("fresh resource must start an acquisition session");
    };
    let (first_reader, first_lease, first_writer) = first.into();
    let first_writer = first_writer.expect("first consumer owns the writer epoch");
    let first_epoch = first_writer.epoch();

    let AcquisitionResult::Pending(follower) = store
        .attach_pending_resource(&key, Arc::new(AtomicU64::new(0)), None)
        .expect("follower attachment")
    else {
        panic!("follower must join the active session");
    };
    let (follower_reader, follower_lease, follower_writer) = follower.into();
    assert!(follower_writer.is_none());

    first_epoch
        .write_at(0, b"old")
        .current()
        .expect("first epoch is current")
        .expect("first write");
    drop(first_writer);
    let next_writer = follower_lease
        .try_take_writer()
        .expect("surviving follower takes writer ownership");
    let next_epoch = next_writer.epoch();

    assert!(
        first_epoch.write_at(3, b"stale").current().is_none(),
        "old epoch write must be stale after handoff"
    );
    assert!(
        first_epoch.commit(Some(5)).current().is_none(),
        "old epoch commit must be stale after handoff"
    );
    assert!(
        first_epoch.relinquish().current().is_none(),
        "old epoch relinquish must be stale after handoff"
    );
    assert!(
        first_epoch
            .fail("late writer failure".to_string())
            .current()
            .is_none(),
        "old epoch fail must be stale after handoff"
    );
    next_epoch
        .write_at(3, b"new")
        .current()
        .expect("new epoch is current")
        .expect("handoff write");
    next_epoch
        .commit(Some(6))
        .current()
        .expect("new epoch is current")
        .expect("handoff commit");

    let mut first_bytes = [0; 6];
    let mut follower_bytes = [0; 6];
    first_reader
        .read_at(0, &mut first_bytes)
        .expect("first reader");
    follower_reader
        .read_at(0, &mut follower_bytes)
        .expect("follower reader");
    assert_eq!(&first_bytes, b"oldnew");
    assert_eq!(&follower_bytes, b"oldnew");
    drop(first_lease);
}

#[kithara::test(timeout(Duration::from_secs(1)))]
fn follower_attach_clones_reader_before_current_failure() {
    let index = PendingResourceIndex::new(CancelToken::never());
    let store = test_store();
    let key = ResourceKey::relative("asset", "attach-fail");
    let (first_lease, first_writer) = attach(&index, &store, &key, entry(0, None));
    let first_writer = first_writer.expect("first writer");
    let first_epoch = first_writer.epoch();
    let attached = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let probe_attached = Arc::clone(&attached);
    let probe_release = Arc::clone(&release);
    index.set_attach_probe_for_test(move || {
        probe_attached.wait();
        probe_release.wait();
    });

    let follower_index = index.clone();
    let follower_store = store.clone();
    let follower_key = key.clone();
    let follower = thread::spawn(move || {
        let remove_store = follower_store.clone();
        let acquire_store = follower_store.clone();
        let acquire_key = follower_key.clone();
        follower_index.attach_pending_resource(
            &follower_key,
            entry(0, None),
            follower_store,
            Arc::new(move |key| remove_store.remove_resource(key)),
            move || acquire_store.acquire_resource(&acquire_key, None),
        )
    });

    attached.wait();
    assert!(
        index.slot_locked_for_test(&key),
        "current failure must wait for the follower's attach critical section"
    );
    let (failed_tx, failed_rx) = mpsc::channel();
    let failure = thread::spawn(move || {
        failed_tx
            .send(first_epoch.fail("current failure".to_string()))
            .expect("failure result receiver");
    });
    assert!(failed_rx.try_recv().is_err());

    release.wait();
    let follower = follower.join().expect("follower attach thread");
    assert!(matches!(follower, Ok(AcquisitionResult::Pending(_))));
    failure.join().expect("failure thread");
    failed_rx
        .recv()
        .expect("failure result")
        .current()
        .expect("first epoch remains current until follower attach completes")
        .expect("failure cleanup");
    drop(first_lease);
}
