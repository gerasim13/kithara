use super::*;

/// A collection page filters its loaded rows by title or artist, ignoring case
/// and surrounding space, without a request, and counts them against the
/// service's total.
#[kithara::test]
async fn a_collection_filter_is_local_and_counts_against_the_service_total() {
    let net = DelayedNet::new(Unimock::new((
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Ok(playlist_reply())),
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Ok(stream_reply())),
    )));
    let mut source = source(net.clone());
    source.select("playlist:1023");
    until(&mut *source, PageStatus::Ready).await;
    assert_eq!(source.read("count"), Some(ReadValue::Text("5 / 215")));
    for (query, track) in [("  birch LANE ", "1003"), ("Eli rook", "1006")] {
        source.write("query", &WriteValue::Text(query.into()));
        assert_eq!(source.rows(None).len(), 1, "{query}");
        assert_eq!(source.row_key(0), Some(track));
        assert_eq!(source.read("count"), Some(ReadValue::Text("1 / 215")));
    }
    source.write("query", &WriteValue::Text(String::new()));
    assert_eq!(source.read("count"), Some(ReadValue::Text("5 / 215")));
    source.tick();
    assert_eq!(source.status(), PageStatus::Ready);
    release(source, &net).await;
    assert_eq!(
        net.events(),
        ["start:node", "end:node", "start:node", "end:node"]
    );
}
