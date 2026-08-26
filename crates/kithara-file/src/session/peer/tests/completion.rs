use super::*;

#[kithara::test]
fn cancelled_invalid_response_relinquishes_without_file_error() {
    let (store, key, inner, writer) = fresh_session(Some(16));
    let epoch = writer.epoch();
    let mut events = inner.source.bus.subscribe();

    inner.complete_fetch(
        &epoch,
        FetchCompletion {
            invalid_response: true,
            ..completion(0, 0, Some(16), Some(&NetError::Cancelled))
        },
    );

    assert!(!writer.is_current());
    assert!(events.try_recv().is_err());
    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Active
    ));
}

#[kithara::test]
fn stale_writer_does_not_report_io_failure() {
    let (_store, _key, inner, writer) = fresh_session(None);
    let epoch = writer.epoch();
    let fetch_cancel = writer.writer_cancel().child();
    assert!(matches!(epoch.relinquish(), WriterOutcome::Current(())));
    let writer = FetchWriter {
        cancel: fetch_cancel.clone(),
        epoch: epoch.clone(),
        inner: Arc::downgrade(&inner),
        invalid_response: Arc::new(AtomicBool::new(false)),
        offset: Arc::new(AtomicU64::new(0)),
    };

    let result = writer.write(b"stale");

    assert!(result.is_ok());
    assert!(fetch_cancel.is_cancelled());
    assert!(!inner.asset.reader.contains_range(0..1));
}

#[kithara::test]
fn transient_after_full_advertised_body_commits() {
    let (store, key, inner, writer) = fresh_session(None);
    let epoch = writer.epoch();
    inner.source.coord.set_total_bytes(Some(4));
    assert!(matches!(
        epoch.write_at(0, b"done"),
        WriterOutcome::Current(Ok(()))
    ));

    inner.finalize_fetch(
        &epoch,
        completion(
            0,
            4,
            None,
            Some(&NetError::Network("tail reset".to_string())),
        ),
    );

    assert_ready_bytes(&store, &key, b"done");
}

#[kithara::test]
fn open_ended_resume_without_total_stays_active() {
    let (store, key, inner, writer) = fresh_session(None);
    let epoch = writer.epoch();
    assert!(matches!(
        epoch.write_at(0, b"old"),
        WriterOutcome::Current(Ok(()))
    ));
    assert!(matches!(
        epoch.write_at(3, b"new"),
        WriterOutcome::Current(Ok(()))
    ));

    inner.finalize_fetch(&epoch, completion(3, 3, None, None));

    assert!(writer.is_current());
    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Active
    ));
}

#[kithara::test]
fn initial_zero_progress_transient_fails_session() {
    let (store, key, inner, writer) = fresh_session(None);
    let epoch = writer.epoch();
    let mut events = inner.source.bus.subscribe();

    inner.finalize_fetch(
        &epoch,
        completion(
            0,
            0,
            None,
            Some(&NetError::Network("connect reset".to_string())),
        ),
    );

    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Missing
    ));
    assert!(matches!(
        events.try_recv(),
        Ok(Envelope {
            event: Event::File(FileEvent::Error { .. }),
            ..
        })
    ));
}

#[kithara::test]
fn finite_watermark_already_present_stays_pending() {
    let (_store, _key, inner, writer) = fresh_session(Some(4));
    assert!(matches!(
        writer.epoch().write_at(0, b"data"),
        WriterOutcome::Current(Ok(()))
    ));
    let peer = make_peer(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
}

#[kithara::test(native, timeout(Duration::from_secs(2)))]
fn inflight_clears_after_completion_settlement() {
    let (_store, _key, inner, writer) = fresh_session(None);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    inner.set_worker_wake(Arc::new(BlockingWake {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    }));
    inner.arm_reader_waker();
    let peer = FilePeer::new(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(mut fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("missing bytes must start a fetch");
    };
    let mut fetch = fetches.remove(0);
    let on_complete = fetch.take_on_complete().expect("completion callback");

    let completion = thread::spawn(move || {
        let error = NetError::Network("initial fetch failed".to_string());
        on_complete(0, None, Some(&error));
    });
    entered.wait();
    let inflight_during_settlement = peer.inflight.lock().is_some();
    release.wait();
    completion
        .join()
        .expect("completion callback must not panic");

    assert!(
        inflight_during_settlement,
        "a replacement fetch must not start before the prior callback settles"
    );
    assert!(peer.inflight.lock().is_none());
}

#[kithara::test(native, timeout(Duration::from_secs(2)))]
fn terminal_reader_wake_settles_file_before_worker_wake() {
    let (_store, _key, inner, writer) = fresh_session(None);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let epoch = writer.epoch();
    assert!(matches!(
        epoch.write_at(0, b"done"),
        WriterOutcome::Current(Ok(()))
    ));
    inner.set_worker_wake(Arc::new(BlockingWake {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    }));
    inner.arm_reader_waker();
    inner.source.coord.set_total_bytes(Some(4));
    let mut events = inner.source.bus.subscribe();

    let completion = thread::spawn(move || epoch.commit(Some(4)));
    entered.wait();
    let mut settled_before_wake = false;
    while let Ok(envelope) = events.try_recv() {
        settled_before_wake |= matches!(
            envelope.event,
            Event::File(FileEvent::CacheComplete { total_bytes: 4 })
        );
    }
    release.wait();
    assert!(matches!(
        completion.join().expect("commit must not panic"),
        WriterOutcome::Current(Ok(()))
    ));

    assert!(
        settled_before_wake,
        "terminal reader wake must settle File state before waking the audio worker"
    );
}

#[kithara::test]
fn repeated_committed_observation_does_not_self_wake() {
    let (_store, _key, inner, writer) = fresh_session(None);
    let wake = Arc::new(CountingWake::default());
    inner.set_worker_wake(Arc::clone(&wake) as Arc<dyn WorkerWake>);
    let epoch = writer.epoch();
    assert!(matches!(
        epoch.write_at(0, b"done"),
        WriterOutcome::Current(Ok(()))
    ));
    assert!(matches!(
        epoch.commit(Some(4)),
        WriterOutcome::Current(Ok(()))
    ));

    assert!(inner.observe_committed());
    let first_count = wake.count();
    assert!(inner.observe_committed());

    assert_eq!(wake.count(), first_count);
}
