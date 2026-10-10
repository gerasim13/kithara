use super::*;

#[kithara::test]
async fn every_row_of_a_repeated_track_gets_its_stream_and_confirmed_reaction() {
    let mut search: serde_json::Value =
        serde_json::from_slice(&search_reply()).expect("the search fixture parses");
    let tracks = search["data"]["search"]["tracks"]["items"]
        .as_array_mut()
        .expect("the search fixture lists tracks");
    tracks[1] = tracks[0].clone();
    let mut streams: serde_json::Value =
        serde_json::from_slice(&stream_reply()).expect("the stream fixture parses");
    streams["data"]["mediaContents"]
        .as_array_mut()
        .expect("the stream fixture lists tracks")
        .remove(1);
    let net = DelayedNet::new(Unimock::new((
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Ok(Bytes::from(
                serde_json::to_vec(&search).expect("the repeated track page encodes"),
            ))),
        NetMock::post_bytes
            .next_call(matching!((_, body, _) if String::from_utf8_lossy(body).matches("1000").count() == 1))
            .returns(Ok(Bytes::from(
                serde_json::to_vec(&streams).expect("the unique stream batch encodes"),
            ))),
        NetMock::post_bytes
            .next_call(matching!(_, _, _))
            .returns(Ok(Bytes::from_static(include_bytes!(
                "../fixtures/like.json"
            )))),
    )));
    let mut source = loaded_search(net.clone()).await;
    assert_eq!(source.rows(None).len(), 5);
    assert!(
        source.rows(None).iter().all(|row| row.drag().is_some()),
        "every row of the repeated track carries its stream"
    );
    source.write("like_track", &WriteValue::Text("1000".into()));
    wait_until(Duration::from_secs(2), "the reaction is confirmed", || {
        source.tick();
        reactions(&*source, "1000").contains(&true)
    })
    .await
    .expect("the repeated track's reaction must be confirmed");
    assert_eq!(reactions(&*source, "1000"), [true, true]);
    release(source, &net).await;
}

#[kithara::test]
async fn unlike_on_liked_page_reloads_confirmed_collection_without_retargeting() {
    let mut reloaded: serde_json::Value =
        serde_json::from_slice(&liked_reply()).expect("the liked reply parses");
    reloaded["data"]["paginatedCollection"]["tracks"]["items"]
        .as_array_mut()
        .expect("the liked reply lists tracks")
        .remove(0);
    let mut streams: serde_json::Value =
        serde_json::from_slice(&stream_reply()).expect("the stream fixture parses");
    streams["data"]["mediaContents"]
        .as_array_mut()
        .expect("the stream fixture lists tracks")
        .remove(0);
    let net = DelayedNet::new(Unimock::new((
        NetMock::post_bytes.next_call(matching!(_, _, _)).returns(Ok(liked_reply())),
        NetMock::post_bytes.next_call(matching!(_, _, _)).returns(Ok(stream_reply())),
        NetMock::post_bytes.next_call(matching!((_, body, _) if serde_json::from_slice::<serde_json::Value>(body).unwrap()["query"].as_str().unwrap().contains(r#"removeItem(id: "1000""#))).returns(Ok(Bytes::from_static(include_bytes!("../fixtures/unlike.json")))),
        NetMock::post_bytes.next_call(matching!((_, body, _) if String::from_utf8_lossy(body).contains("KitharaLiked"))).returns(Ok(Bytes::from(serde_json::to_vec(&reloaded).unwrap()))),
        NetMock::post_bytes.next_call(matching!(_, _, _)).returns(Ok(Bytes::from(serde_json::to_vec(&streams).unwrap()))),
    )));
    let mut source = source(net.clone()).await;
    source.select("liked");
    until(&mut *source, PageStatus::Ready).await;
    assert_eq!(source.row_key(0), Some("1000"));
    assert_eq!(reactions(&*source, "1000"), [true]);
    source.write("like_track", &WriteValue::Text("1000".into()));
    assert_eq!(source.rows(None).len(), 5);
    assert_eq!(reactions(&*source, "1000"), [true]);
    wait_until(Duration::from_secs(2), "the collection reloads", || {
        source.tick();
        source.status() == PageStatus::Ready && source.rows(None).len() == 4
    })
    .await
    .expect("the confirmed unlike must reload the liked collection");
    assert_eq!(source.row_key(0), Some("1003"));
    assert!(reactions(&*source, "1000").is_empty());
    source.write("like_track", &WriteValue::Text("1000".into()));
    source.tick();
    assert_eq!(source.rows(None).len(), 4);
    release(source, &net).await;
    assert_eq!(net.events().len(), 10);
}
