use super::*;

#[kithara::test]
fn remote_capture_metadata_publishes_opened() {
    let (_store, _key, inner, _writer) = fresh_session(None);
    let mut rx = inner.source.bus.subscribe();
    let mut headers = Headers::default();
    headers.insert("content-type", "audio/mpeg");
    headers.insert("content-length", "12");

    assert!(inner.capture_content_metadata(&headers, 0, None));

    assert!(matches!(
        rx.try_recv(),
        Ok(Envelope {
            event: Event::File(FileEvent::TotalBytesResolved { .. }),
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(Envelope {
            event: Event::File(FileEvent::Opened {
                cached: false,
                total_bytes: Some(12),
                ..
            }),
            ..
        })
    ));
}

#[kithara::test]
fn conflicting_response_total_is_rejected_before_body() {
    let (_store, _key, inner, _writer) = fresh_session(None);
    inner.source.coord.set_total_bytes(Some(100));
    let mut headers = Headers::default();
    headers.insert("content-range", "bytes 16-31/200");
    headers.insert("content-length", "16");

    assert!(!inner.capture_content_metadata(&headers, 16, Some(32)));
    assert_eq!(inner.source.coord.total_bytes(), Some(100));
    assert!(!inner.asset.reader.contains_range(0..1));
}

#[kithara::test]
fn marking_complete_publishes_cache_complete_once() {
    let (_store, _key, inner, _writer) = fresh_session(None);
    let mut rx = inner.source.bus.subscribe();
    inner.source.coord.set_total_bytes(Some(12));

    inner.mark_complete();

    assert!(matches!(
        rx.try_recv(),
        Ok(Envelope {
            event: Event::File(FileEvent::CacheComplete { total_bytes: 12 }),
            ..
        })
    ));
    inner.mark_complete();
    assert!(rx.try_recv().is_err());
}

#[kithara::test]
fn bounded_response_uses_content_range_total() {
    let mut headers = Headers::default();
    headers.insert("content-length", "16");
    headers.insert("content-range", "bytes 0-15/60");

    assert_eq!(response_contract(&headers, 0, Some(16)).total, Some(60));

    let mut length_only = Headers::default();
    length_only.insert("content-length", "16");
    assert_eq!(response_contract(&length_only, 32, Some(48)).total, None);
    assert_eq!(response_contract(&length_only, 32, None).total, None);
    assert!(response_contract(&length_only, 32, None).invalid);
}

#[kithara::test]
fn range_ignored_at_zero_uses_full_length_and_commits() {
    let (store, key, inner, writer) = fresh_session(Some(16));
    let mut headers = Headers::default();
    headers.insert("content-length", "6");
    inner.capture_content_metadata(&headers, 0, Some(16));
    assert_eq!(inner.source.coord.total_bytes(), Some(6));
    let epoch = writer.epoch();
    assert!(matches!(
        epoch.write_at(0, b"all!!!"),
        WriterOutcome::Current(Ok(()))
    ));

    inner.finalize_fetch(&epoch, completion(0, 6, Some(16), None));

    assert_ready_bytes(&store, &key, b"all!!!");
}

#[kithara::test]
fn resumed_response_without_content_range_fails_before_write() {
    let (store, key, inner, writer) = fresh_session(Some(16));
    let epoch = writer.epoch();
    let mut headers = Headers::default();
    headers.insert("content-length", "6");
    let contract = response_contract(&headers, 3, Some(19));
    assert!(contract.invalid);
    let fetch_cancel = writer.writer_cancel().child();
    let writer = FetchWriter {
        cancel: fetch_cancel,
        epoch: epoch.clone(),
        inner: Arc::downgrade(&inner),
        invalid_response: Arc::new(AtomicBool::new(contract.invalid)),
        offset: Arc::new(AtomicU64::new(3)),
    };

    let result = writer.write(b"all!!!");

    assert!(result.is_err());
    assert!(!inner.asset.reader.contains_range(3..4));
    inner.fail_current_epoch(
        &epoch,
        "bounded response did not identify the requested range at offset 3".to_string(),
    );
    assert!(matches!(
        store.resource_state(&key).expect("resource state"),
        AssetResourceState::Missing
    ));
}

#[kithara::test]
fn bounded_response_without_size_is_invalid() {
    assert!(response_contract(&Headers::default(), 0, Some(16)).invalid);
}

#[kithara::test]
fn content_range_without_content_length_is_invalid() {
    let mut headers = Headers::default();
    headers.insert("content-range", "bytes 0-15/60");

    assert!(response_contract(&headers, 0, Some(16)).invalid);
}

#[kithara::test]
fn content_range_without_known_total_is_invalid() {
    let mut headers = Headers::default();
    headers.insert("content-length", "8");
    headers.insert("content-range", "bytes 8-15/*");

    assert!(response_contract(&headers, 8, Some(16)).invalid);
}

#[kithara::test]
fn content_range_end_at_total_is_invalid() {
    let mut headers = Headers::default();
    headers.insert("content-range", "bytes 0-60/60");

    assert!(response_contract(&headers, 0, Some(61)).invalid);
}

#[kithara::test]
fn content_range_must_match_requested_interval() {
    let mut wrong_start = Headers::default();
    wrong_start.insert("content-range", "bytes 1-15/60");
    assert!(response_contract(&wrong_start, 0, Some(16)).invalid);

    let mut past_end = Headers::default();
    past_end.insert("content-range", "bytes 0-16/60");
    assert!(response_contract(&past_end, 0, Some(16)).invalid);

    let mut wrong_length = Headers::default();
    wrong_length.insert("content-range", "bytes 0-15/60");
    wrong_length.insert("content-length", "15");
    assert!(response_contract(&wrong_length, 0, Some(16)).invalid);
}
