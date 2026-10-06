#![cfg(not(target_arch = "wasm32"))]

//! Two queue entries may name the same URL: a playlist that repeats a
//! track, or one asset reachable under a single address. A player event
//! must resolve to the entry that actually played. Resolving it by
//! source alone answers with whichever entry holds that URL first,
//! which is the wrong track as soon as the second copy is the one
//! playing.

use std::num::NonZeroUsize;

use kithara::{
    assets::AssetStore,
    audio::{AudioConfig, DecodeErrorKind, NoResamplerBackend, TrackFailureKind},
    events::{SlotId, TrackId},
    file::{File, FileConfig, FileSrc},
    platform::{sync::Arc, tokio::sync::broadcast::error::TryRecvError},
    play::{ItemRole, PlaybackFault, PlayerEvent, Resource, TrackRef},
    queue::{QueueControl, QueueEvent, TrackStatus, Transition},
};
use kithara_integration_tests::{
    event::TestEvent,
    kithara,
    offline::{OfflinePlayer, append_source_loaded, asset_source, offline_queue_fixture},
};
use kithara_test_fixtures::{asset::Asset, assets, signal::rms};
use kithara_test_utils::cancel_token;

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
/// ≈ 0.74 s of rendered audio — far short of the 30 s track.
const WARMUP_BLOCKS: usize = 64;
const EOF_BLOCK_BUDGET: usize = 128;

async fn render_loop(
    queue: &QueueControl<TestPools>,
    harness: &OfflinePlayer,
    block_budget: usize,
) -> Vec<f32> {
    let mut output = Vec::with_capacity(block_budget * BLOCK_FRAMES * 2);
    for block in 0..block_budget {
        harness
            .run(queue, QueueControl::tick)
            .await
            .expect("tick queue before rendering");
        let samples = harness.render(BLOCK_FRAMES).await;
        assert_eq!(samples.len(), BLOCK_FRAMES * 2, "stereo block {block}");
        assert!(
            samples.iter().all(|sample| sample.is_finite()),
            "master PCM must be finite in block {block}"
        );
        output.extend(samples);
    }
    output
}

fn status_of(queue: &QueueControl<TestPools>, id: TrackId) -> TrackStatus {
    queue
        .track(id)
        .map(|entry| entry.status)
        .expect("the entry must still be in the queue")
}

/// Two entries appended from one file, with the second one selected.
struct SecondCopyPlaying {
    harness: OfflinePlayer,
    queue: QueueControl<TestPools>,
    source: String,
    first: TrackId,
    playing: TrackId,
}

async fn fixture_playing_the_second_copy(track: &Asset) -> SecondCopyPlaying {
    let (harness, queue) = offline_queue_fixture(SAMPLE_RATE).await;
    let source = asset_source(track);
    let first = append_source_loaded(&harness, &queue, source.clone()).await;
    let playing = append_source_loaded(&harness, &queue, source.clone()).await;

    harness
        .run(&queue, move |q| q.select(playing, Transition::None))
        .await
        .expect("select the second copy");

    SecondCopyPlaying {
        harness,
        queue,
        source,
        first,
        playing,
    }
}

#[kithara::test(tokio, flash(false))]
#[case::played_entry(true)]
#[case::same_url_entry(false)]
async fn a_failure_only_flags_the_entry_that_played(#[case] played_entry: bool) {
    let SecondCopyPlaying {
        harness,
        queue,
        source,
        first,
        playing,
    } = fixture_playing_the_second_copy(&assets::constant_wav_loud_30s()).await;
    render_loop(&queue, &harness, WARMUP_BLOCKS).await;

    harness
        .player()
        .bus()
        .publish(TestEvent::Player(PlayerEvent::ItemDidFail {
            item: ItemRole::Leading(TrackRef::new(playing, SlotId::new(0), Arc::from(source))),
            fault: PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            }),
        }));
    render_loop(&queue, &harness, WARMUP_BLOCKS).await;

    let id = if played_entry { playing } else { first };
    let status = status_of(&queue, id);
    assert_eq!(
        matches!(status, TrackStatus::Failed(_)),
        played_entry,
        "only the entry that played may be flagged: {status:?}"
    );
    drop(queue);
    harness.close().await;
}

#[kithara::test(tokio, flash(false))]
async fn a_real_source_cancellation_reaches_only_its_queue_entry_once() {
    let asset = assets::constant_wav_loud_30s();
    let SecondCopyPlaying {
        harness,
        queue,
        source,
        first,
        playing,
    } = fixture_playing_the_second_copy(&asset).await;
    let source_cancel = cancel_token();
    let pools = harness.worker().pools().clone();
    let path = asset.path().expect("native WAV fixture path").to_owned();
    let file = FileConfig::for_src(FileSrc::Local(path))
        .store(AssetStore::builder(pools.clone()).build())
        .pools(pools)
        .cancel(source_cancel.clone())
        .build();
    let config = AudioConfig::<File<TestPools>, NoResamplerBackend>::for_stream(file)
        .audio_buffer_chunks(2)
        .preload_chunks(NonZeroUsize::MIN)
        .build();
    assert!(
        config.cancel().is_none(),
        "only the file source owns this cancellation"
    );
    let audio = harness.worker().load(config).await.expect("real file lane");
    let mut resource = Resource::from_reader(audio, Some(Arc::from(source.clone())));
    resource
        .preload()
        .await
        .expect("the real lane has produced PCM");
    harness
        .run(harness.player(), move |player| {
            player
                .replace_item(1, resource, playing)
                .expect("replace selected queue entry");
        })
        .await;
    harness.run(&queue, QueueControl::play).await;
    let samples = render_loop(&queue, &harness, WARMUP_BLOCKS).await;
    for (block, samples) in samples.chunks_exact(BLOCK_FRAMES * 2).enumerate() {
        let level = rms(samples);
        assert!(
            level > 0.0,
            "real source master PCM must be nonzero before cancellation in block {block}: rms={level}"
        );
    }
    assert_eq!(
        harness
            .player()
            .rt_metrics()
            .expect("the real source owns an active render slot")
            .underruns(),
        0,
        "the pre-cancellation render must not conceal producer starvation"
    );
    assert!(
        harness.player().is_playing(),
        "the real source must still be playing before cancellation"
    );
    assert!(
        harness.position() > 0.0,
        "the selected real source must have played"
    );
    assert_eq!(queue.current().map(|entry| entry.id), Some(playing));
    assert_eq!(status_of(&queue, playing), TrackStatus::Loaded);
    let sibling_status = status_of(&queue, first);
    let mut events = queue.subscribe::<TestEvent>();

    source_cancel.cancel();
    render_loop(&queue, &harness, WARMUP_BLOCKS).await;
    harness
        .run(&queue, QueueControl::tick)
        .await
        .expect("tick queue after source cancellation");

    let fault = PlaybackFault::Source(TrackFailureKind::SourceCancelled);
    let reason = fault.to_string();
    let status = TrackStatus::Failed(reason.clone());
    assert_eq!(status_of(&queue, playing), status);
    assert_eq!(status_of(&queue, first), sibling_status);
    assert!(
        queue.current().is_none(),
        "the failed final entry ends the queue"
    );
    let mut observed = Vec::new();
    loop {
        match events.try_recv() {
            Ok(envelope) => observed.push(envelope.event),
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(skipped)) => panic!("lost {skipped} terminal events"),
        }
    }
    let failures: Vec<_> = observed
        .iter()
        .filter_map(|event| match event {
            TestEvent::Player(PlayerEvent::ItemDidFail { item, fault }) => Some((item, *fault)),
            _ => None,
        })
        .collect();
    let [(item, actual)] = failures.as_slice() else {
        panic!("one real player failure must reach the queue: {failures:?}");
    };
    assert!(item.is_leading());
    assert_eq!(item.track().id, playing);
    assert_eq!(item.track().src.as_ref(), source);
    assert_eq!(*actual, fault);
    assert_eq!(
        observed
            .iter()
            .filter(|event| matches!(
                event,
                TestEvent::Queue(QueueEvent::TrackStatusChanged { id, status: actual })
                    if *id == playing && *actual == status
            ))
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| matches!(
                event,
                TestEvent::Queue(QueueEvent::TrackLoadFailed { id, reason: actual, auto_skipped: true })
                    if *id == playing && *actual == reason
            ))
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| matches!(event, TestEvent::Queue(QueueEvent::QueueEnded)))
            .count(),
        1
    );
    assert!(
        observed.iter().all(|event| !matches!(
            event,
            TestEvent::Player(PlayerEvent::ItemDidPlayToEnd { .. })
        )),
        "source cancellation must never publish natural EOF"
    );
    drop(queue);
    harness.close().await;
}

#[kithara::test(tokio, flash(false))]
async fn second_entry_with_the_same_source_owns_its_real_eof() {
    let SecondCopyPlaying {
        harness,
        queue,
        source: _source,
        first,
        playing,
    } = fixture_playing_the_second_copy(&assets::constant_wav_loud_0_5s()).await;
    render_loop(&queue, &harness, EOF_BLOCK_BUDGET).await;

    assert_eq!(
        status_of(&queue, playing),
        TrackStatus::Consumed,
        "the selected duplicate must own its natural EOF"
    );
    assert_eq!(
        status_of(&queue, first),
        TrackStatus::Consumed,
        "the initially selected duplicate remains distinct from the entry that reached EOF"
    );
    assert!(
        queue.current().is_none(),
        "queue must be inactive after its terminal EOF"
    );

    drop(queue);
    harness.close().await;
}
