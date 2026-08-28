use std::num::NonZeroU32;

use kithara_bufpool::PcmPool;
use kithara_decode::PcmChunk;
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
use kithara_platform::sync::Arc;
#[cfg(feature = "analysis-waveform")]
use kithara_resampler::NoResamplerBackend;
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
use kithara_resampler::rubato::RubatoBackend;
use kithara_test_utils::kithara;
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
use unimock::{MockFn, Unimock, matching};

use super::rebuild::{Consts, RouteFixture, TestStream, route_signal_source_with_effects};
#[cfg(feature = "analysis-waveform")]
use crate::{
    analysis::AnalyzerBuilder,
    coverage::{Coverage, FrameRange},
};
use crate::{
    analysis::producer::{AnalysisProducer, ring},
    pipeline::{source::StreamAudioSource, track::TrackStep},
    renderer::AudioWorkerSource,
    traits::AudioEffect,
};
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
use crate::{
    analysis::{
        GridState,
        beat::{BeatDetector, BeatDetectorMock, BeatMark, GridParams, RawBeats},
    },
    waveform::BeatGrid,
};

/// Chunks to pull. Well inside the transport, so nothing is refused and the
/// two runs are comparable.
const CHUNKS: usize = 12;

/// A scripted effect whose output must never reach the analysis pass: it
/// scales every sample and drops half of every chunk.
struct GainAndHalve;

impl AudioEffect for GainAndHalve {
    fn flush(&mut self) -> Option<PcmChunk> {
        None
    }

    fn process(&mut self, mut chunk: PcmChunk) -> Option<PcmChunk> {
        for sample in chunk.samples.iter_mut() {
            *sample *= 0.25;
        }
        let frames = chunk.meta.frames / 2;
        let samples = usize::try_from(frames)
            .ok()?
            .checked_mul(usize::from(chunk.meta.spec.channels))?;
        chunk.samples.truncate(samples);
        chunk.meta.frames = frames;
        Some(chunk)
    }

    fn reset(&mut self) {}
}

/// One playback run: the chunks it produced and what the pass was offered,
/// each range's source start with its mono frames.
struct Offered {
    produced: usize,
    ranges: Vec<(u64, Vec<f32>)>,
}

impl Offered {
    /// Every produced chunk is offered exactly once, so an equal count is a
    /// refusal count of zero without threading the outcome out of the step.
    fn refusals(&self) -> usize {
        self.produced.saturating_sub(self.ranges.len())
    }
}

/// Pull chunks until `target` of them are produced, running `after_step` once
/// per step, and report how many were produced and how many steps it took.
fn drive(
    source: &mut StreamAudioSource<TestStream>,
    target: usize,
    mut after_step: impl FnMut(),
) -> (usize, usize) {
    let mut produced = 0;
    let mut steps = 0;
    for _ in 0..target.saturating_mul(4).max(1024) {
        if produced >= target {
            break;
        }
        source.rebuild.run_inline();
        source.flush_deferred();
        steps += 1;
        match source.step_track() {
            TrackStep::Produced(_) => produced += 1,
            TrackStep::StateChanged | TrackStep::Blocked(_) => {}
            TrackStep::Eof | TrackStep::Failed => break,
        }
        after_step();
    }
    (produced, steps)
}

/// Drain everything the transport holds into `out`.
fn drain(reader: &mut ring::Reader, out: &mut Vec<(u64, Vec<f32>)>) {
    let mut buffer = PcmPool::default().get_with(Vec::clear);
    while let Some(at) = reader.pop(&mut buffer) {
        out.push((at, buffer.to_vec()));
    }
}

/// Decode `CHUNKS` chunks through `effects` with a pass attached, and return
/// what the pass was offered.
async fn offered(effects: Vec<Box<dyn AudioEffect>>) -> Offered {
    offered_for(effects, CHUNKS).await
}

/// Decode `target` chunks through `effects` with a pass attached, draining the
/// transport as playback goes so a long run is never refused for want of room.
async fn offered_for(effects: Vec<Box<dyn AudioEffect>>, target: usize) -> Offered {
    let rate = NonZeroU32::new(Consts::SAMPLE_RATE).expect("test rate is non-zero");
    let (writer, mut reader) = ring::open_for(rate);
    let producer = AnalysisProducer::new(writer, rate, "route-track".into());

    let RouteFixture { source, .. } =
        route_signal_source_with_effects(Consts::SAMPLE_RATE, effects).await;
    let mut source = source.with_analysis(producer);

    let mut ranges = Vec::new();
    let (produced, _) = drive(&mut source, target, || drain(&mut reader, &mut ranges));
    drain(&mut reader, &mut ranges);
    Offered { produced, ranges }
}

#[kithara::test(native, tokio)]
async fn the_effect_chain_does_not_reach_the_analysis_pass() {
    let plain = offered(Vec::new()).await;
    let processed = offered(vec![Box::new(GainAndHalve)]).await;

    assert!(
        !plain.ranges.is_empty(),
        "the harness must offer something at all"
    );
    assert_eq!(
        plain.ranges, processed.ranges,
        "the offer is taken before the chain runs, so the ranges are identical"
    );
    assert_eq!(
        (plain.refusals(), processed.refusals()),
        (0, 0),
        "a refusal would change the push sequence, so neither run may have one"
    );
}

#[kithara::test(native, tokio)]
async fn offered_ranges_are_positioned_and_contiguous() {
    let offers = offered(Vec::new()).await;

    let mut at = None;
    for (start, mono) in &offers.ranges {
        if let Some(previous) = at {
            assert_eq!(
                *start, previous,
                "a decoded range starts where the last one ended"
            );
        }
        assert!(!mono.is_empty(), "an empty range is never offered");
        at = Some(start + u64::try_from(mono.len()).unwrap_or(0));
    }
    assert!(at.is_some_and(|end| end > 0), "the pass was fed something");
}

#[kithara::test(native, tokio)]
async fn a_track_with_no_pass_offers_nothing() {
    let RouteFixture { mut source, .. } =
        route_signal_source_with_effects(Consts::SAMPLE_RATE, Vec::new()).await;

    let (produced, _) = drive(&mut source, CHUNKS, || {});

    assert_eq!(produced, CHUNKS, "playback runs with no handle attached");
    assert!(source.analysis.is_none(), "and none was created for it");
}

/// Playback must not wait on a pass that cannot keep up, and what it could not
/// hand over must not pass for analysed.
///
/// There is no decode tick budget to measure, so "unaffected" is output parity
/// against a run with no handle: same steps, same chunks, same ranges. The
/// timing and allocation half is RTSan's, and no lane attaches a handle yet.
/// The transport is drained here as the worker drains it on its tick.
#[cfg(feature = "analysis-waveform")]
#[kithara::test(native, tokio)]
async fn a_saturated_pass_leaves_playback_alone_and_reports_what_it_missed() {
    // Blocks held at once, with the sample side set far above any chunk this
    // fixture decodes so the descriptor side is what refuses. Two phases of
    // `PHASE` chunks each: `SPANS` are taken and the rest refused.
    const SPANS: usize = 3;
    const SAMPLES: usize = 1 << 16;
    const PHASE: usize = CHUNKS / 2;

    let rate = NonZeroU32::new(Consts::SAMPLE_RATE).expect("test rate is non-zero");
    let ample = offered(Vec::new()).await;
    let RouteFixture {
        source: mut bare, ..
    } = route_signal_source_with_effects(Consts::SAMPLE_RATE, Vec::new()).await;
    let baseline = drive(&mut bare, 2 * PHASE, || {});

    let RouteFixture { source, .. } =
        route_signal_source_with_effects(Consts::SAMPLE_RATE, Vec::new()).await;
    let (writer, mut reader) = ring::open(SAMPLES, SPANS);
    let mut source =
        source.with_analysis(AnalysisProducer::new(writer, rate, "route-track".into()));

    // Saturate: nothing drains while the first phase decodes, so the ranges
    // past the transport's capacity are refused.
    let (first, first_steps) = drive(&mut source, PHASE, || {});
    let mut taken = Vec::new();
    drain(&mut reader, &mut taken);
    assert_eq!(
        taken.len(),
        SPANS,
        "the transport takes what it holds and no more"
    );

    // Carry on past the hole, so the refusal sits inside the coverage rather
    // than behind the frontier where nothing can be claimed about it.
    let (second, second_steps) = drive(&mut source, PHASE, || {});
    drain(&mut reader, &mut taken);

    assert_eq!(
        (first + second, first_steps + second_steps),
        baseline,
        "a saturated pass costs playback no chunk and no extra step"
    );
    assert_eq!(
        first + second,
        ample.produced,
        "and decodes as much as it does with a transport that takes everything"
    );
    for range in &taken {
        assert!(
            ample.ranges.contains(range),
            "a range that lands under saturation is the one the ample run \
             decoded, unshifted and unaltered: start {}",
            range.0
        );
    }

    let mut analyzers = AnalyzerBuilder::<NoResamplerBackend>::default()
        .with_waveform(64)
        .build(rate, "route-track".into());
    for (at, mono) in &taken {
        analyzers.push_mono(mono, *at, None);
    }
    let analysis = analyzers.snapshot(None, true);

    let frontier = analysis.extent().unwrap_or(0);
    let block =
        |(at, mono): &(u64, Vec<f32>)| FrameRange::new(*at, u64::try_from(mono.len()).unwrap_or(0));
    let reached = taken.last().map(block).map_or(0, FrameRange::end);
    assert_eq!(
        frontier, reached,
        "end of stream pins the extent to the last range that landed"
    );

    let mut refused = Coverage::default();
    for range in ample.ranges.iter().map(block) {
        if range.end() <= frontier && !analysis.coverage().contains(range) {
            refused.insert(range);
        }
    }
    assert!(
        !refused.runs().is_empty(),
        "the run must actually refuse something to be worth asserting on"
    );
    assert_eq!(
        analysis.missing(),
        refused.runs(),
        "what the transport refused is what the pass reports missing"
    );

    let edges: Vec<u64> = ample
        .ranges
        .iter()
        .map(block)
        .flat_map(|range| [range.start(), range.end()])
        .collect();
    for range in analysis.missing() {
        assert!(
            edges.contains(&range.start()) && edges.contains(&range.end()),
            "a whole range is refused or none of it: {range:?} splits a decoded block"
        );
    }
}

/// Both artifacts of one pass: the waveform's bytes, and the grid with what it
/// says about itself.
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
type Artifacts = (Vec<u8>, Option<(BeatGrid, GridState, Vec<FrameRange>)>);

/// Fold `ranges` into one pass the way the analysis worker folds what it
/// drains, and return both its artifacts.
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
fn artifacts(ranges: &[(u64, Vec<f32>)]) -> Artifacts {
    /// Every window reports one beat a quarter of the way in, so a marker's
    /// position is a pure function of where its window sits.
    fn detector() -> Box<dyn BeatDetector> {
        Box::new(Unimock::new(
            BeatDetectorMock
                .each_call(matching!(_))
                .answers_arc(Arc::new(|_, _| {
                    Ok(RawBeats {
                        beats: vec![BeatMark::at(0.25)],
                        downbeats: vec![BeatMark::at(0.25)],
                    })
                })),
        ))
    }

    let rate = NonZeroU32::new(Consts::SAMPLE_RATE).expect("test rate is non-zero");
    let mut builder = AnalyzerBuilder::<RubatoBackend>::default()
        .with_waveform(64)
        .with_beat_detector(detector(), GridParams::default());
    let mut beat = builder.take_detector();
    let mut analyzers = builder.build(rate, "route-track".into());

    for (at, mono) in ranges {
        analyzers.push_mono(mono, *at, beat.as_mut());
    }
    let snapshot = analyzers.snapshot(beat.as_mut(), true);

    (
        snapshot.waveform().map(Vec::<u8>::from).unwrap_or_default(),
        snapshot.beat().map(|beat| {
            (
                beat.grid().clone(),
                beat.state(),
                beat.unanalysed().to_vec(),
            )
        }),
    )
}

/// The other half of effects transparency: the ranges are the same either way
/// (`the_effect_chain_does_not_reach_the_analysis_pass`), so what the pass
/// makes of them must be too, artifact for artifact.
///
/// Neither run may refuse a block: a dropped block changes the push sequence
/// and with it the segmentation the artifacts fall out of.
#[cfg(all(feature = "analysis-beat", feature = "analysis-waveform"))]
#[kithara::test(native, tokio)]
async fn the_same_range_sequence_yields_the_same_artifacts() {
    // Source long enough for the pass to publish a grid at all; a waveform is
    // meaningful at any length, a grid is not.
    const SECONDS: usize = 2;

    let target =
        SECONDS * usize::try_from(Consts::SAMPLE_RATE).unwrap_or(0) / Consts::ROUTE_CHUNK_FRAMES;
    let plain = offered_for(Vec::new(), target).await;
    let processed = offered_for(vec![Box::new(GainAndHalve)], target).await;

    assert_eq!(
        (plain.refusals(), processed.refusals()),
        (0, 0),
        "a refused block would leave the two runs comparing different inputs"
    );

    let want = artifacts(&plain.ranges);
    assert!(!want.0.is_empty(), "the harness must produce a waveform");
    assert!(want.1.is_some(), "and must publish a grid");
    assert_eq!(
        artifacts(&processed.ranges),
        want,
        "the chain changed what was played, not what was analysed"
    );
}

/// A pass that ended leaves its transport with no reader. Playback must stop
/// paying for it rather than copying into a ring nobody drains.
#[kithara::test(native, tokio)]
async fn the_step_stops_offering_once_the_pass_has_ended() {
    let rate = NonZeroU32::new(Consts::SAMPLE_RATE).expect("test rate is non-zero");
    let (writer, reader) = ring::open_for(rate);
    let RouteFixture { source, .. } =
        route_signal_source_with_effects(Consts::SAMPLE_RATE, Vec::new()).await;
    let mut source =
        source.with_analysis(AnalysisProducer::new(writer, rate, "route-track".into()));

    drive(&mut source, 1, || {});
    assert!(
        source.analysis.is_some(),
        "the pass is open, so the handle is"
    );

    drop(reader);
    drive(&mut source, 1, || {});
    assert!(
        source.analysis.is_none(),
        "the handle goes when the pass it feeds does"
    );
}
