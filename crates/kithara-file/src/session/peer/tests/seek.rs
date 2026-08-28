use super::*;

/// A forward seek moves the reader cursor past everything already stored. The
/// next fetch has to start there. That is what a range request buys: the
/// listener waits for the bytes under the cursor, not for the span they
/// skipped over.
#[kithara::test]
fn a_forward_seek_fetches_from_the_new_cursor() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    assert!(matches!(
        writer.epoch().write_at(0, &[0u8; 512]),
        WriterOutcome::Current(Ok(()))
    ));

    inner.source.coord.set_position(3072);

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    let PeerAction::Fetch(plan) = peer.next_action(&inner, lease) else {
        panic!("a resource missing bytes under the cursor must fetch");
    };

    assert_eq!(plan.start, 3072);
}

/// Everything ahead of the cursor is stored, so the peer has nothing to serve
/// the listener and falls back to filling the span an earlier seek skipped.
#[kithara::test]
fn a_cursor_with_no_gap_ahead_backfills_the_skipped_span() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    assert!(matches!(
        writer.epoch().write_at(3072, &[0u8; 1024]),
        WriterOutcome::Current(Ok(()))
    ));

    inner.source.coord.set_position(3072);

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    let PeerAction::Fetch(plan) = peer.next_action(&inner, lease) else {
        panic!("a resource missing its head must fetch");
    };

    assert_eq!(plan.start, 0);
}

/// A running fetch streams forward from where it started, so a seek that lands
/// past its write cursor cannot be served by it — it would have to deliver the
/// whole skipped span first. The peer cancels it instead.
#[kithara::test]
fn a_seek_past_the_download_cursor_cancels_the_running_fetch() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    let peer = FilePeer::new(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());

    let Poll::Ready(Some(fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("a resource missing every byte must start a fetch");
    };
    let running = fetches[0]
        .cancel()
        .expect("a peer fetch carries its own cancel")
        .clone();

    inner
        .source
        .coord
        .seek_control()
        .begin(Duration::from_secs(30));
    inner.source.coord.set_position(3072);

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
    assert!(running.is_cancelled());
}

/// The peer parks on its own waker while a fetch is in flight, so the
/// completion that clears the in-flight slot is the only thing left that can
/// bring it back to plan the replacement fetch.
#[kithara::test]
fn a_settled_fetch_wakes_the_parked_peer() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    let peer = FilePeer::new(&inner, Some(writer));
    let wake = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut cx = Context::from_waker(&waker);

    let Poll::Ready(Some(mut fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("a resource missing every byte must start a fetch");
    };
    let on_complete = fetches
        .remove(0)
        .take_on_complete()
        .expect("completion callback");

    inner.source.coord.set_position(3072);
    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
    let parked = wake.count();

    on_complete(0, None, Some(&NetError::Cancelled));

    assert!(wake.count() > parked);
}

/// The write offset a fetch publishes lags the bytes it lands: the storage
/// wakes the reader inside `write_at`, the offset is stored after. A reader
/// that consumed those bytes in between sits ahead of the offset without being
/// ahead of the fetch, so the fetch is not overtaken and must keep running.
#[kithara::test]
fn a_reader_consuming_landed_bytes_does_not_cancel_the_fetch() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    let epoch = writer.epoch();
    let peer = FilePeer::new(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());

    let Poll::Ready(Some(fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("a resource missing every byte must start a fetch");
    };
    let running = fetches[0]
        .cancel()
        .expect("a peer fetch carries its own cancel")
        .clone();

    assert!(matches!(
        epoch.write_at(0, &[0u8; 512]),
        WriterOutcome::Current(Ok(()))
    ));
    inner.source.coord.set_position(256);

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
    assert!(!running.is_cancelled());
}

/// A cancelled fetch relinquishes without committing and its replacement is
/// planned from the resource it left behind. When it had already landed
/// everything, the plan finds no gap: the writer commits instead of parking,
/// or every consumer waits on a resource that is complete.
#[kithara::test]
fn a_writer_with_nothing_left_to_fetch_commits() {
    let (store, key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4));
    assert!(matches!(
        writer.epoch().write_at(0, b"done"),
        WriterOutcome::Current(Ok(()))
    ));

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    assert!(matches!(peer.next_action(&inner, lease), PeerAction::Done));
    assert_ready_bytes(&store, &key, b"done");
}

/// A plan bounded by the demand watermark can run out of gaps while bytes
/// beyond it are still missing. That is a wait for demand, not a complete
/// resource: committing there would truncate it.
#[kithara::test]
fn a_plan_bounded_by_demand_does_not_commit_a_partial_resource() {
    let (store, key, inner, writer) = fresh_session(Some(512));
    inner.source.coord.set_total_bytes(Some(4096));
    assert!(matches!(
        writer.epoch().write_at(0, &[0u8; 512]),
        WriterOutcome::Current(Ok(()))
    ));

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    assert!(matches!(
        peer.next_action(&inner, lease),
        PeerAction::Pending
    ));
    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Active
    ));
}

/// A backfill fetch starts behind the cursor by construction — it fills what an
/// earlier seek skipped. The cursor being ahead of it is the normal case, not a
/// reason to cancel it, or the peer would kill every backfill it starts.
#[kithara::test]
fn a_backfill_fetch_survives_a_cursor_ahead_of_it() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    assert!(matches!(
        writer.epoch().write_at(3072, &[0u8; 1024]),
        WriterOutcome::Current(Ok(()))
    ));
    inner.source.coord.set_position(3072);

    let peer = FilePeer::new(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());

    let Poll::Ready(Some(fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("a resource missing its head must fetch");
    };
    let backfill = fetches[0]
        .cancel()
        .expect("a peer fetch carries its own cancel")
        .clone();

    inner.source.coord.set_position(4096);

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
    assert!(!backfill.is_cancelled());
}

/// Before the first response has answered how long the resource is, a cursor
/// past the end is indistinguishable from one inside it. Anchoring a fetch
/// there would ask for bytes that may not exist and would replace the very
/// request that reports the real length, so the cursor stays out of it.
#[kithara::test]
fn a_cursor_does_not_steer_before_the_extent_is_known() {
    let (_store, _key, inner, writer) = fresh_session(None);
    assert!(matches!(
        writer.epoch().write_at(0, &[0u8; 512]),
        WriterOutcome::Current(Ok(()))
    ));

    inner.source.coord.set_position(3072);

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    let PeerAction::Fetch(plan) = peer.next_action(&inner, lease) else {
        panic!("a resource of unknown extent must fetch");
    };

    assert_eq!(plan.start, 512);
}

/// A cursor past a known extent has no bytes to ask for, so it steers nothing
/// and the peer keeps filling the resource it does have.
#[kithara::test]
fn a_cursor_past_the_known_extent_does_not_steer() {
    let (_store, _key, inner, writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(4096));
    assert!(matches!(
        writer.epoch().write_at(0, &[0u8; 512]),
        WriterOutcome::Current(Ok(()))
    ));

    inner.source.coord.set_position(9000);

    let peer = make_peer(&inner, Some(writer));
    let lease = inner.resource_lease.as_ref().expect("session lease");

    let PeerAction::Fetch(plan) = peer.next_action(&inner, lease) else {
        panic!("a resource missing bytes must fetch");
    };

    assert_eq!(plan.start, 512);
}

/// The fetch that establishes the extent must survive a cursor that cannot
/// steer: cancelling it would hand the replacement the same anchor it already
/// has, and the length would never arrive.
#[kithara::test]
fn a_fetch_of_unknown_extent_survives_a_seek() {
    let (_store, _key, inner, writer) = fresh_session(None);
    let peer = FilePeer::new(&inner, Some(writer));
    let mut cx = Context::from_waker(Waker::noop());

    let Poll::Ready(Some(fetches)) = Peer::poll_next(&peer, &mut cx) else {
        panic!("a resource missing every byte must start a fetch");
    };
    let head = fetches[0]
        .cancel()
        .expect("a peer fetch carries its own cancel")
        .clone();

    inner.source.coord.set_position(3072);

    assert!(matches!(Peer::poll_next(&peer, &mut cx), Poll::Pending));
    assert!(!head.is_cancelled());
}
