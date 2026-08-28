#[cfg(feature = "analysis-waveform")]
use std::num::NonZeroU32;

#[cfg(feature = "analysis-waveform")]
use kithara_bufpool::SamplePool;
#[cfg(feature = "analysis-beat")]
use kithara_platform::sync::Arc;
use kithara_platform::{CancelToken, sync::mpsc, tokio::sync::watch};
#[cfg(feature = "analysis-beat")]
use kithara_resampler::rubato::RubatoBackend;
use kithara_resampler::{NoResamplerBackend, ResamplerBackend};
#[cfg(feature = "analysis-waveform")]
use kithara_signal::AudioSpec;
use kithara_test_utils::kithara;
#[cfg(feature = "analysis-beat")]
use num_traits::cast::ToPrimitive;
#[cfg(feature = "analysis-beat")]
use unimock::{MockFn, Unimock, matching};

#[cfg(feature = "analysis-beat")]
use super::super::beat::{BeatDetector, BeatDetectorMock, BeatMark, GridParams, RawBeats};
use super::{
    super::{
        analyzer::{AnalyzerBuilder, GridState, TrackAnalysis},
        worker::{AnalysisNode, AnalysisStep, Job},
    },
    fixtures::{CH, FakeReader, SR, sine},
};
#[cfg(feature = "analysis-waveform")]
use crate::analysis::producer::{AnalysisProducer, Offer, ring};
#[cfg(feature = "analysis-waveform")]
use crate::coverage::FrameRange;
use crate::traits::AudioReader;
#[cfg(feature = "analysis-waveform")]
use crate::waveform::{AnalysisParams, WaveformAnalyzer};

#[cfg(feature = "analysis-waveform")]
const BUCKETS: usize = 64;

#[cfg(feature = "analysis-waveform")]
fn waveform_only() -> AnalyzerBuilder<NoResamplerBackend> {
    AnalyzerBuilder::<NoResamplerBackend>::default().with_waveform(BUCKETS)
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn pending_reader_yields_one_scheduler_tick() {
    let (jobs, receiver) = mpsc::channel();
    let (tx, _results) = watch::channel(None);
    jobs.send(Job {
        token: "test-track".into(),
        tx,
        rate: super::fixtures::spec().sample_rate,
        ingest: super::fixtures::idle_ingest(),
        reader: Box::new(FakeReader::chunked_with_pending(&sine(1024), 1)),
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);

    assert_eq!(node.tick(), AnalysisStep::UpstreamPending);
    assert_eq!(node.tick(), AnalysisStep::Progress);
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn cancel_racing_finalize_drops_sender_without_emitting() {
    let (jobs, receiver) = mpsc::channel();
    let (tx, results) = watch::channel(None);
    let cancel = CancelToken::root();
    jobs.send(Job {
        token: "test-track".into(),
        tx,
        rate: super::fixtures::spec().sample_rate,
        ingest: super::fixtures::idle_ingest(),
        reader: Box::new(FakeReader::chunked(&sine(1024), 1)),
        cancel: cancel.clone(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);

    assert_eq!(node.tick(), AnalysisStep::Progress, "decode one chunk");
    assert_eq!(node.tick(), AnalysisStep::Progress, "EOF arms finalize");
    cancel.cancel();
    assert_eq!(node.tick(), AnalysisStep::Progress, "cancel drops the task");
    assert!(results.borrow().is_none());
    assert!(results.has_changed().is_err(), "task sender is dropped");
}

#[cfg(feature = "analysis-waveform")]
fn offered(ranges: &[(u64, usize)]) -> Option<TrackAnalysis> {
    let rate = super::fixtures::spec().sample_rate;
    let (jobs, receiver) = mpsc::channel();
    let (tx, results) = watch::channel(None);
    let (writer, ingest) = ring::open_for(rate);
    let mut producer = AnalysisProducer::new(writer, rate, "test-track".into());
    jobs.send(Job {
        token: "test-track".into(),
        reader: Box::new(FakeReader::stalled(ranges.len() + 2)),
        tx,
        rate,
        ingest,
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);

    for (at, frames) in ranges {
        assert_eq!(
            producer.offer(&sine(*frames), super::fixtures::spec(), *at),
            Offer::Taken,
            "the transport takes a range on its own axis"
        );
    }

    for _ in 0..128 {
        let _ = node.tick();
    }
    // The watch keeps the last publication even once the task's sender is
    // gone, so this reads the final snapshot either way.
    results.borrow().clone()
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn offered_ranges_land_where_they_were_offered() {
    let analysis = offered(&[(0, 1024), (4096, 1024)]).expect("the pass publishes");

    assert_eq!(
        analysis.coverage().runs(),
        &[FrameRange::new(0, 1024), FrameRange::new(4096, 1024)],
        "coverage is what was offered, at the positions it was offered at"
    );
    assert!(
        analysis.waveform().is_some(),
        "a pass fed only by a producer still produces artifacts"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn an_offer_reaches_only_the_pass_its_handle_names() {
    let rate = super::fixtures::spec().sample_rate;
    let open = |token: &str| {
        let (jobs, receiver) = mpsc::channel();
        let (tx, results) = watch::channel(None);
        let (writer, ingest) = ring::open_for(rate);
        jobs.send(Job {
            token: token.into(),
            reader: Box::new(FakeReader::stalled(8)),
            tx,
            rate,
            ingest,
            cancel: CancelToken::root(),
        })
        .expect("analysis node accepts the test job");
        (
            jobs,
            AnalysisNode::new(waveform_only(), receiver),
            results,
            AnalysisProducer::new(writer, rate, token.into()),
        )
    };

    let (_fed_jobs, mut fed_node, fed_results, mut producer) = open("track-a");
    let (_idle_jobs, mut idle_node, idle_results, _idle_producer) = open("track-b");

    assert_eq!(
        producer.offer(&sine(1024), super::fixtures::spec(), 0),
        Offer::Taken
    );
    for _ in 0..64 {
        let _ = fed_node.tick();
        let _ = idle_node.tick();
    }

    let fed = fed_results
        .borrow()
        .clone()
        .expect("the fed pass publishes");
    assert_eq!(fed.token().as_str(), "track-a");
    assert_eq!(
        fed.coverage().runs(),
        &[FrameRange::new(0, 1024)],
        "the pass the handle names covers what it was offered"
    );
    assert!(
        idle_results.borrow().is_none(),
        "the pass no one offered to covers nothing and publishes nothing"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn an_offer_on_another_axis_leaves_the_coverage_alone() {
    let rate = super::fixtures::spec().sample_rate;
    let foreign = AudioSpec {
        channels: CH,
        sample_rate: NonZeroU32::new(48_000).expect("test rate is non-zero"),
    };
    let (jobs, receiver) = mpsc::channel();
    let (tx, results) = watch::channel(None);
    let (writer, ingest) = ring::open_for(rate);
    let mut producer = AnalysisProducer::new(writer, rate, "test-track".into());
    jobs.send(Job {
        token: "test-track".into(),
        reader: Box::new(FakeReader::stalled(4)),
        tx,
        rate,
        ingest,
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);

    assert_eq!(
        producer.offer(&sine(1024), super::fixtures::spec(), 0),
        Offer::Taken
    );
    assert_eq!(
        producer.offer(&sine(1024), foreign, 4096),
        Offer::ForeignRate,
        "the mismatch is reported to the producer"
    );

    for _ in 0..128 {
        let _ = node.tick();
    }
    let analysis = results.borrow().clone().expect("the pass publishes");
    assert_eq!(
        analysis.coverage().runs(),
        &[FrameRange::new(0, 1024)],
        "only the range on the pass's own axis is covered"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn a_pass_fed_by_a_producer_publishes_as_it_goes() {
    const BLOCK: u64 = 8192;
    const BLOCKS: u64 = 90;
    const STALLS: usize = 400;

    let rate = super::fixtures::spec().sample_rate;
    let (jobs, receiver) = mpsc::channel();
    let (tx, mut results) = watch::channel(None);
    let (writer, ingest) = ring::open_for(rate);
    let mut producer = AnalysisProducer::new(writer, rate, "test-track".into());
    jobs.send(Job {
        token: "test-track".into(),
        reader: Box::new(FakeReader::stalled(STALLS)),
        tx,
        rate,
        ingest,
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);
    let pcm = sine(usize::try_from(BLOCK).unwrap_or(0));

    let mut published = Vec::new();
    let collect = |results: &mut watch::Receiver<Option<TrackAnalysis>>,
                   out: &mut Vec<TrackAnalysis>| {
        if results.has_changed().is_ok_and(|changed| changed)
            && let Some(analysis) = results.borrow_and_update().clone()
        {
            out.push(analysis);
        }
    };

    for block in 0..BLOCKS {
        assert_eq!(
            producer.offer(&pcm, super::fixtures::spec(), block * BLOCK),
            Offer::Taken,
            "the worker keeps the transport drained"
        );
        for _ in 0..4 {
            let _ = node.tick();
            collect(&mut results, &mut published);
        }
    }
    let mid = published.len();
    for _ in 0..STALLS {
        let _ = node.tick();
        collect(&mut results, &mut published);
    }
    // The task drops its sender the moment it finishes, so its last
    // publication is readable from the watch but never reported as a
    // change. Take it from the value itself.
    let last = results.borrow().clone().expect("the pass publishes");
    if published
        .last()
        .is_none_or(|prev| prev.revision() < last.revision())
    {
        published.push(last.clone());
    }

    assert!(
        mid >= 2,
        "the pass publishes while coverage grows, not only at the end: {mid} before EOF"
    );
    assert!(
        published
            .windows(2)
            .all(|pair| pair[1].revision() > pair[0].revision()),
        "each publication outranks the last: {:?}",
        published
            .iter()
            .map(TrackAnalysis::revision)
            .collect::<Vec<_>>()
    );
    for early in published.iter().take(mid) {
        assert!(
            early.extent().is_none() && !early.is_complete(),
            "a publication made while coverage grows is provisional"
        );
        assert!(
            early.waveform().is_some(),
            "and it carries the artifact it is worth publishing for"
        );
    }
    assert_eq!(
        last.coverage().runs(),
        &[FrameRange::new(0, BLOCKS * BLOCK)],
        "everything offered is covered by the end: publications={}, revisions={:?}, missing={:?}",
        published.len(),
        published
            .iter()
            .map(TrackAnalysis::revision)
            .collect::<Vec<_>>(),
        last.missing()
    );
}

#[cfg(feature = "analysis-waveform")]
fn refusal_run(reoffer: bool) -> (TrackAnalysis, FrameRange, u64) {
    const BLOCK: u64 = 8192;
    const PAST: u64 = 40;
    // Enough stalls that the reader outlives every offer below.
    const STALLS: usize = 200;

    let rate = super::fixtures::spec().sample_rate;
    let (jobs, receiver) = mpsc::channel();
    let (tx, results) = watch::channel(None);
    let (writer, ingest) = ring::open_for(rate);
    let mut producer = AnalysisProducer::new(writer, rate, "test-track".into());
    jobs.send(Job {
        token: "test-track".into(),
        reader: Box::new(FakeReader::stalled(STALLS)),
        tx,
        rate,
        ingest,
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);
    let pcm = sine(usize::try_from(BLOCK).unwrap_or(0));

    let mut at = 0;
    let refused = loop {
        match producer.offer(&pcm, super::fixtures::spec(), at) {
            Offer::Taken => at = at.saturating_add(BLOCK),
            Offer::Full => break FrameRange::new(at, BLOCK),
            other => panic!("a range on the pass axis is taken or refused, got {other:?}"),
        }
    };

    for block in 1..PAST {
        for _ in 0..4 {
            let _ = node.tick();
        }
        assert_eq!(
            producer.offer(
                &pcm,
                super::fixtures::spec(),
                refused.start() + block * BLOCK
            ),
            Offer::Taken,
            "a drained transport takes the next range"
        );
    }
    if reoffer {
        for _ in 0..4 {
            let _ = node.tick();
        }
        assert_eq!(
            producer.offer(&pcm, super::fixtures::spec(), refused.start()),
            Offer::Taken,
            "the transport has room for the range it refused"
        );
    }
    for _ in 0..STALLS {
        let _ = node.tick();
    }

    let analysis = results.borrow().clone().expect("the pass publishes");
    (analysis, refused, refused.start() + PAST * BLOCK)
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn a_range_the_transport_refused_is_reported_missing() {
    let (analysis, refused, reached) = refusal_run(false);

    assert!(
        analysis.missing().contains(&refused),
        "the refused range is missing: {refused:?} not in {:?}",
        analysis.missing()
    );
    assert!(
        !analysis.coverage().contains(refused),
        "a refused range is not covered"
    );
    assert_eq!(
        analysis.coverage().runs(),
        &[
            FrameRange::new(0, refused.start()),
            FrameRange::new(refused.end(), reached - refused.end()),
        ],
        "the hole splits the coverage in two"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn a_refused_range_offered_again_leaves_the_missing_set() {
    let (analysis, refused, reached) = refusal_run(true);

    assert!(
        analysis.missing().is_empty(),
        "the range was taken on the second offer: still missing {:?}",
        analysis.missing()
    );
    assert!(
        analysis.coverage().contains(refused),
        "and it is covered now"
    );
    assert_eq!(
        analysis.coverage().runs(),
        &[FrameRange::new(0, reached)],
        "entering the coverage once leaves one contiguous run"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn a_seek_order_pass_keeps_publishing_and_covers_the_union() {
    const BLOCK: u64 = 8192;
    const BLOCKS: u64 = 90;
    const STALLS: usize = 400;

    // The listener starts halfway in, then seeks back to the opening.
    let order: Vec<u64> = (BLOCKS / 2..BLOCKS).chain(0..BLOCKS / 2).collect();

    let rate = super::fixtures::spec().sample_rate;
    let (jobs, receiver) = mpsc::channel();
    let (tx, mut results) = watch::channel(None);
    let (writer, ingest) = ring::open_for(rate);
    let mut producer = AnalysisProducer::new(writer, rate, "test-track".into());
    jobs.send(Job {
        token: "test-track".into(),
        reader: Box::new(FakeReader::stalled(STALLS)),
        tx,
        rate,
        ingest,
        cancel: CancelToken::root(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(waveform_only(), receiver);
    let pcm = sine(usize::try_from(BLOCK).unwrap_or(0));

    let mut published: Vec<TrackAnalysis> = Vec::new();
    for block in &order {
        assert_eq!(
            producer.offer(&pcm, super::fixtures::spec(), block * BLOCK),
            Offer::Taken,
            "the worker keeps the transport drained"
        );
        for _ in 0..4 {
            let _ = node.tick();
            if results.has_changed().is_ok_and(|changed| changed)
                && let Some(analysis) = results.borrow_and_update().clone()
            {
                published.push(analysis);
            }
        }
    }
    for _ in 0..STALLS {
        let _ = node.tick();
    }
    let last = results.borrow().clone().expect("the pass publishes");

    assert!(
        published.len() >= 2,
        "the pass publishes while coverage grows: {} publications",
        published.len()
    );
    assert!(
        published
            .windows(2)
            .all(|pair| pair[1].revision() > pair[0].revision()),
        "each publication outranks the last: {:?}",
        published
            .iter()
            .map(TrackAnalysis::revision)
            .collect::<Vec<_>>()
    );
    assert!(
        last.revision() > published.last().map_or(0, TrackAnalysis::revision),
        "and so does the last one"
    );
    assert!(
        published.first().is_some_and(|first| first
            .coverage()
            .runs()
            .first()
            .is_some_and(|run| run.start() > 0)),
        "the seek reached the pass: the first publication does not start at zero"
    );
    assert_eq!(
        last.coverage().runs(),
        &[FrameRange::new(0, BLOCKS * BLOCK)],
        "coverage is the union of everything offered"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn offers_out_of_order_cover_their_union() {
    let ascending = offered(&[(0, 1024), (1024, 1024), (2048, 1024)]);
    let shuffled = offered(&[(2048, 1024), (0, 1024), (1024, 1024)]);

    let ascending = ascending.expect("the ascending pass publishes");
    let shuffled = shuffled.expect("the shuffled pass publishes");
    assert_eq!(
        ascending.coverage().runs(),
        &[FrameRange::new(0, 3072)],
        "three touching ranges are one run"
    );
    assert_eq!(
        shuffled.coverage(),
        ascending.coverage(),
        "arrival order does not change what is covered"
    );
}

fn stages<B>(
    reader: Box<dyn AudioReader>,
    builder: AnalyzerBuilder<B>,
    cancel: &CancelToken,
) -> Vec<TrackAnalysis>
where
    B: ResamplerBackend,
{
    let (jobs, receiver) = mpsc::channel();
    let (tx, mut results) = watch::channel(None);
    jobs.send(Job {
        token: "test-track".into(),
        reader,
        tx,
        rate: super::fixtures::spec().sample_rate,
        ingest: super::fixtures::idle_ingest(),
        cancel: cancel.clone(),
    })
    .expect("analysis node accepts the test job");
    let mut node = AnalysisNode::new(builder, receiver);
    let mut out = Vec::new();
    for _ in 0..128 {
        let _ = node.tick();
        match results.has_changed() {
            Ok(true) => {
                if let Some(analysis) = results.borrow_and_update().clone() {
                    out.push(analysis);
                }
            }
            Ok(false) => {}
            Err(_) => {
                if let Some(analysis) = results.borrow_and_update().clone() {
                    out.push(analysis);
                }
                break;
            }
        }
    }
    out
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn matches_direct_waveform_analyzer_over_chunked_stream() {
    let samples = sine(usize::try_from(SR).unwrap());
    let frames = u64::try_from(samples.len() / usize::from(CH)).unwrap_or(0);
    let mut direct = WaveformAnalyzer::new(SR, AnalysisParams::default(), &SamplePool::default());
    direct.push(&samples, usize::from(CH), 0);
    let want = direct.snapshot(BUCKETS, Some(frames));

    let reader = Box::new(FakeReader::chunked(&samples, 4));
    let out = stages(reader, waveform_only(), &CancelToken::root());
    assert_eq!(out.len(), 1, "waveform-only emits once");
    let got = out[0]
        .waveform()
        .cloned()
        .expect("waveform analyzer fills its slot");
    assert_eq!(
        Vec::<u8>::from(&got),
        Vec::<u8>::from(&want),
        "worker path must reproduce the direct analyzer output"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn cancelled_token_yields_none() {
    let cancel = CancelToken::root();
    cancel.cancel();
    let reader = Box::new(FakeReader::chunked(&sine(4096), 2));
    assert!(stages(reader, waveform_only(), &cancel).is_empty());
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn decode_error_yields_none() {
    let reader = Box::new(FakeReader::failing());
    let out = stages(reader, waveform_only(), &CancelToken::root());
    assert!(out.is_empty());
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn empty_stream_yields_none() {
    let reader = Box::new(FakeReader::empty());
    let out = stages(reader, waveform_only(), &CancelToken::root());
    assert!(out.is_empty(), "EOF with no chunks is not an analysis");
}

#[cfg(feature = "analysis-beat")]
#[kithara::test]
fn beat_slot_fills_the_beat_grid() {
    let raw = RawBeats {
        beats: Vec::new(),
        downbeats: (0..9u8).map(|n| BeatMark::at(f32::from(n) * 2.0)).collect(),
    };
    let mock = Unimock::new(
        BeatDetectorMock
            .next_call(matching!(_))
            .answers_arc(Arc::new(move |_, _| Ok(raw.clone()))),
    );
    let detector = Box::new(mock) as Box<dyn BeatDetector>;
    let builder = AnalyzerBuilder::<RubatoBackend>::default()
        .with_beat_detector(detector, GridParams::default());

    let reader = Box::new(FakeReader::chunked(
        &sine(17 * usize::try_from(SR).unwrap()),
        3,
    ));
    let out = stages(reader, builder, &CancelToken::root());
    assert!(
        out.len() >= 2,
        "17 s of source outlives one publication interval, got {} publication(s)",
        out.len()
    );
    let revisions: Vec<u64> = out.iter().map(TrackAnalysis::revision).collect();
    assert!(
        revisions.windows(2).all(|pair| pair[1] > pair[0]),
        "each publication must outrank the last: {revisions:?}"
    );
    assert!(
        out.iter()
            .any(|analysis| analysis.beat().is_some() && analysis.extent().is_none()),
        "a grid must reach a consumer before the extent is known"
    );
    // Every stage between the detector and the snapshot can drop a marker's
    // confidence, and each is unit-tested on its own. This is the one place
    // that proves a mark survives all of them still carrying one.
    let marked = out
        .iter()
        .filter_map(TrackAnalysis::beat)
        .find(|beat| {
            beat.grid()
                .beat_confidence()
                .iter()
                .chain(beat.grid().downbeat_confidence())
                .any(Option::is_some)
        })
        .expect("some publication carries a marker the detector reported");
    assert_eq!(
        marked.confidence(),
        Some(0.9),
        "the number the detector reported survives every stage between it and \
         the published snapshot"
    );
    assert!(
        out.iter()
            .filter(|analysis| analysis.extent().is_none())
            .all(|analysis| analysis
                .beat()
                .is_none_or(|beat| beat.state() == GridState::Provisional)),
        "a grid published mid-decode cannot claim to be final"
    );
    let last = out.last().expect("at least one publication");
    assert_eq!(
        last.extent(),
        Some(u64::from(SR) * 17),
        "end of stream pins the extent to what was covered"
    );
    let grid = last
        .beat()
        .cloned()
        .expect("beat slot fills its slot in the final publication");
    assert!(
        (grid.grid().bpm() - 120.0).abs() < 1e-6,
        "2 s bars are 120 bpm, got {}",
        grid.grid().bpm()
    );
    // The reported tempo must describe the markers riding the same
    // revision, not a value derived from something already replaced.
    let downbeats = grid.grid().downbeats();
    let mut gaps: Vec<u64> = downbeats
        .windows(2)
        .filter_map(|pair| pair[1].checked_sub(pair[0]))
        .collect();
    gaps.sort_unstable();
    let bar_frames = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
    let bar_seconds = bar_frames.to_f64().unwrap_or(1.0) / f64::from(SR);
    let bpm_from_marks = 4.0 * 60.0 / bar_seconds;
    assert!(
        (bpm_from_marks - grid.grid().bpm()).abs() < 1e-6,
        "bpm {} must describe the published markers ({bpm_from_marks} from bars)",
        grid.grid().bpm()
    );
    assert_eq!(
        grid.grid().downbeats()[1],
        u64::from(SR) * 2,
        "source frames"
    );
}

#[cfg(feature = "analysis-waveform")]
#[kithara::test]
fn pending_is_tolerated_mid_stream() {
    let samples = sine(8192);
    let reader = Box::new(FakeReader::chunked_with_pending(&samples, 2));
    let out = stages(reader, waveform_only(), &CancelToken::root());
    assert!(out.len() == 1 && out[0].waveform().is_some());
}
