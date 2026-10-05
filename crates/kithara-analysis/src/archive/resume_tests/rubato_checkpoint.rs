use std::{
    num::{NonZeroU32, NonZeroU64, NonZeroUsize},
    ops::Range,
};

use kithara_audio::{AudioControl, AudioRead, AudioSession, ChunkOutcome, ReadOutcome, SeekOutcome};
use kithara_beat::BeatDetectError;
use kithara_decode::{DecodeError, TrackMetadata};
use kithara_events::EventBus;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
    time::Duration,
    tokio::sync::watch,
};
use kithara_resampler::{MonoStream, MonoStreamConfig, ResamplerOptions, rubato::RubatoBackend};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCoverage};
use kithara_test_utils::kithara;

use super::{AnalysisFile, AnalysisFileSpec, apply};
use crate::{
    AnalysisProgress, AnalyzerBuilder, BeatAnalysisConfig, BeatState,
    analyzer::{AnalysisDemand, Detector, Extent, Ingest},
    beat::{BeatDetector, BeatMark, GridParams, RawBeats},
    producer::ring,
    test_pools::{Pools, TestPools, pools, sample_buffer},
    worker::{AnalysisTask, Job},
};

const SOURCE_RATE: u32 = 48_000;
const DETECTOR_RATE: u32 = 22_050;
const ORIGIN: u64 = 1_105;
const END: u64 = ORIGIN + 6 * SOURCE_RATE as u64 + 257;

type Heard = Arc<Mutex<Vec<Vec<f32>>>>;

struct RecordingDetector(Heard);

impl BeatDetector for RecordingDetector {
    fn detect(&self, mono: &[f32]) -> Result<RawBeats, BeatDetectError> {
        self.0.lock().push(mono.to_vec());
        Ok(RawBeats::new(vec![BeatMark::new(0.005, 0.9)], Vec::new()))
    }
}

fn spec() -> AudioSpec {
    AudioSpec::new(
        1,
        NonZeroU32::new(SOURCE_RATE).expect("source rate is non-zero"),
    )
}

fn detector_boundary(source: u64) -> usize {
    usize::try_from(
        (u128::from(source) * u128::from(DETECTOR_RATE) + u128::from(SOURCE_RATE / 2))
            / u128::from(SOURCE_RATE),
    )
    .expect("fixture detector coordinate fits usize")
}

fn config() -> BeatAnalysisConfig<RubatoBackend> {
    BeatAnalysisConfig::builder()
        .resampler_backend(RubatoBackend::default())
        .target_rate(DETECTOR_RATE)
        .block_frames(1024)
        .detector_window_seconds(2)
        .detector_overlap_seconds(0)
        .detector_min_window_seconds(1)
        .build()
}

fn configured(pools: Pools) -> (AnalyzerBuilder<RubatoBackend, TestPools>, Detector, Heard) {
    let heard = Heard::default();
    let mut builder = AnalyzerBuilder::<RubatoBackend, _>::new(pools)
        .with_waveform(8)
        .with_beat_config(config())
        .with_beat_detector(
            Box::new(RecordingDetector(Arc::clone(&heard))),
            GridParams::default(),
        );
    let detector = builder
        .take_detector()
        .expect("recording detector is configured");
    (builder, detector, heard)
}

fn mono_stream(pools: Pools) -> MonoStream<RubatoBackend> {
    let config = config();
    MonoStream::new(
        MonoStreamConfig::builder()
            .backend(RubatoBackend::default())
            .source_sample_rate(spec().sample_rate)
            .target_sample_rate(NonZeroU32::new(DETECTOR_RATE).expect("detector rate"))
            .quality(config.resampler_quality())
            .options(
                ResamplerOptions::builder()
                    .chunk_size(config.block_frames())
                    .build(),
            )
            .pools(pools)
            .build(),
    )
    .expect("real Rubato stream opens")
}

fn decoded(pools: &Pools, pcm: &[f32], range: Range<u64>) -> AudioChunk {
    let start = usize::try_from(range.start).expect("fixture source start fits");
    let end = usize::try_from(range.end).expect("fixture source end fits");
    AudioChunk::new(
        AudioChunkInfo {
            spec: spec(),
            frames: u32::try_from(end - start).expect("fixture chunk fits u32"),
            frame_offset: range.start,
            ..Default::default()
        },
        sample_buffer(pools, &pcm[start..end]),
    )
}

struct Source {
    pcm: Vec<f32>,
    pools: Pools,
    reads: Arc<Mutex<Vec<Range<u64>>>>,
    seeks: Arc<Mutex<Vec<u64>>>,
    at: u64,
    bus: EventBus,
    metadata: TrackMetadata,
}

impl AudioSession for Source {
    fn duration(&self) -> Option<Duration> {
        Some(
            spec()
                .duration_for(END)
                .expect("fixture extent has a duration"),
        )
    }

    fn event_bus(&self) -> &EventBus {
        &self.bus
    }

    fn metadata(&self) -> &TrackMetadata {
        &self.metadata
    }
}

impl AudioRead for Source {
    fn spec(&self) -> AudioSpec {
        spec()
    }

    fn position(&self) -> Duration {
        spec()
            .duration_for(self.at)
            .expect("fixture cursor has a duration")
    }

    fn next_chunk(&mut self) -> Result<ChunkOutcome, DecodeError> {
        if self.at >= END {
            return Ok(ChunkOutcome::Eof {
                position: self.position(),
            });
        }
        let range = self.at..END;
        self.at = range.end;
        self.reads.lock().push(range.clone());
        Ok(ChunkOutcome::Chunk(decoded(&self.pools, &self.pcm, range)))
    }

    fn read(&mut self, _output: &mut [f32]) -> Result<ReadOutcome, DecodeError> {
        unreachable!("the analysis scheduler uses decoded chunks")
    }

    fn read_planar<'a>(
        &mut self,
        _output: &'a mut [&'a mut [f32]],
    ) -> Result<ReadOutcome, DecodeError> {
        unreachable!("the analysis scheduler uses decoded chunks")
    }
}

impl AudioControl for Source {
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, DecodeError> {
        let target = spec()
            .frame_at(position)
            .expect("fixture seek fits source axis");
        self.seeks.lock().push(target);
        if target < ORIGIN {
            return Err(DecodeError::InvalidData {
                detail: "fixture has no decodable head",
            });
        }
        self.at = target;
        if target >= END {
            return Ok(SeekOutcome::PastEof {
                target: position,
                duration: self.duration().expect("fixture extent is known"),
            });
        }
        Ok(SeekOutcome::Landed {
            target: position,
            landed_at: self.position(),
        })
    }
}

#[kithara::test(native, flash(false))]
fn off_grid_rubato_checkpoint_preserves_real_pcm_and_refills_only_its_pending_tail() {
    let pools = pools();
    let pcm: Vec<f32> = (0..END)
        .map(|frame| {
            0.25 + f32::from(u16::try_from((frame / 97) % 31).expect("watermark fits")) / 512.0
        })
        .collect();
    let chunk_frames = NonZeroU64::new(u64::from(SOURCE_RATE)).expect("one-second chunk");
    let (builder, detector, _) = configured(pools.clone());
    let mut live = builder
        .build(
            spec().sample_rate,
            "rubato-off-grid".into(),
            0,
            AnalysisDemand::ALL,
        )
        .expect("analysis buffers fit");
    assert_eq!(
        live.push(
            &decoded(&pools, &pcm, ORIGIN..END),
            &mut Extent::default(),
            None,
        ),
        Ingest::Accepted,
    );
    for _ in 0..2 {
        let request = live
            .prepare_detection(false)
            .expect("two full windows are ready");
        live.apply_detection(request.detect(detector.as_ref()));
    }
    let progress = live.progress(None, false, chunk_frames, Some(END));
    let saved_beats = progress
        .analysis()
        .beat()
        .expect("checkpoint beat slot")
        .artifact()
        .beats()
        .to_vec();

    let mut stream = mono_stream(pools.clone());
    let source_start = usize::try_from(ORIGIN).expect("fixture source origin fits");
    let mut materialized = Vec::new();
    stream
        .push(pcm[source_start..].iter().copied(), |samples| {
            materialized.extend_from_slice(samples);
        })
        .expect("real Rubato input is accepted");
    let detector_end = detector_boundary(ORIGIN) + materialized.len();
    let durable_end = u64::try_from(detector_end).expect("detector end fits")
        * u64::from(SOURCE_RATE)
        / u64::from(DETECTOR_RATE);
    assert!(
        durable_end > ORIGIN && durable_end < END,
        "a live tail is not yet materialized",
    );
    let actual_len = materialized.len();
    let mut completed = materialized.clone();
    stream
        .finish(|samples| completed.extend_from_slice(samples))
        .expect("the real tail drains");
    assert_eq!(
        completed.len(),
        detector_boundary(END) - detector_boundary(ORIGIN),
        "the fixture's local oracle count equals its absolute endpoint count",
    );
    assert!(
        completed.len() > actual_len,
        "the backend still owned real output at capture",
    );
    assert!(
        completed[actual_len..]
            .iter()
            .all(|sample| sample.abs() > 0.05),
        "pending PCM is nonzero",
    );

    let file_spec = AnalysisFileSpec::new(
        spec().sample_rate,
        END,
        chunk_frames,
        progress.analysis().fingerprint().clone(),
    )
    .expect("archive spec matches the pass");
    let bytes = apply(&AnalysisFile::create(&file_spec, &progress).expect("checkpoint update"));
    let file = AnalysisFile::parse(&bytes, progress.analysis().fingerprint())
        .expect("real archive parses");
    let restored_progress: AnalysisProgress = file.into();
    let resume = restored_progress
        .decode_resume()
        .expect("resume bytes decode")
        .expect("unsettled resume");
    let beat = resume.beat.expect("beat checkpoint is present");
    let run = beat.runs.first().expect("released run retains its suffix");
    assert_eq!(beat.runs.len(), 1);
    assert!(
        run.start > ORIGIN,
        "successful full windows released a nonzero source prefix",
    );
    assert_eq!(
        run.end, durable_end,
        "only emitted PCM is durable; live source admission is retired",
    );
    let first = detector_boundary(run.start) - detector_boundary(ORIGIN);
    let last = detector_boundary(durable_end) - detector_boundary(ORIGIN);
    assert_eq!(
        run.mono.as_ref(),
        &materialized[first..last],
        "retained PCM keeps its exact coordinate and values",
    );
    assert!(
        restored_progress.analysis().coverage().covers(&(ORIGIN..END)),
        "source observation stays historical",
    );
    assert!(
        beat.taken.covers(&(ORIGIN..durable_end)),
        "completed results and real retained PCM remain admitted",
    );
    assert!(
        !beat.taken.covers(&(durable_end..END)),
        "the scheduler can reread the non-durable tail",
    );
    let released = ORIGIN..run.start;
    let completed_beats: Vec<_> = saved_beats
        .into_iter()
        .filter(|beat| released.contains(beat))
        .collect();
    assert!(
        !completed_beats.is_empty(),
        "completed nonzero source ranges already carry successful marks",
    );

    let fresh_start = usize::try_from(durable_end).expect("fresh source origin fits");
    let mut fresh = mono_stream(pools.clone());
    let mut fresh_suffix = Vec::new();
    fresh
        .push(pcm[fresh_start..].iter().copied(), |samples| {
            fresh_suffix.extend_from_slice(samples);
        })
        .expect("a fresh same-backend segment accepts real source PCM");
    fresh
        .finish(|samples| fresh_suffix.extend_from_slice(samples))
        .expect("a fresh same-backend segment drains");
    assert_eq!(
        fresh_suffix.len(),
        detector_boundary(END) - detector_boundary(durable_end),
        "the fresh oracle's local count equals its absolute endpoint count",
    );
    assert!(
        fresh_suffix.iter().all(|sample| sample.abs() > 0.05),
        "refill must contain real source PCM",
    );

    let (resumed_builder, mut resumed_detector, resumed_heard) = configured(pools.clone());
    let reads = Arc::new(Mutex::new(Vec::new()));
    let seeks = Arc::new(Mutex::new(Vec::new()));
    let source = Source {
        pcm,
        pools: pools.clone(),
        reads: Arc::clone(&reads),
        seeks: Arc::clone(&seeks),
        at: ORIGIN,
        bus: EventBus::default(),
        metadata: TrackMetadata::default(),
    };
    let (_writer, ingest) =
        ring::open_for(&pools, spec().sample_rate).expect("ingest ring fits");
    let (tx, results) = watch::channel(None);
    let job = Job {
        demand: AnalysisDemand::ALL,
        token: restored_progress.analysis().token().clone(),
        revision: restored_progress.analysis().revision(),
        reader: Box::new(source),
        cancel: CancelToken::root(),
        rate: spec().sample_rate,
        resume: Some(restored_progress),
        ingest,
        tx,
    };
    let mut task = AnalysisTask::new(
        job,
        &resumed_builder,
        NonZeroU32::new(1).expect("chunk seconds"),
        NonZeroUsize::new(8).expect("drain limit"),
        NonZeroU32::new(1).expect("publish seconds"),
    )
    .expect("builder restores the parsed checkpoint through the real task");
    for _ in 0..32 {
        if task.is_done() {
            break;
        }
        task.tick(&resumed_builder, Some(&mut resumed_detector));
    }
    assert!(
        task.is_done(),
        "the bounded source and real trailing work settle",
    );
    assert_eq!(
        &*reads.lock(),
        &[durable_end..END],
        "only the non-durable tail is decoded again",
    );
    assert!(
        seeks.lock().contains(&durable_end),
        "the real schedule admits the observed but retired tail",
    );
    assert!(
        resumed_heard
            .lock()
            .last()
            .expect("trailing detection hears the refill")
            .ends_with(&fresh_suffix),
        "restored suffix equals a fresh same-backend segment, not fabricated silence",
    );
    let publication = results.borrow();
    let analysis = publication.as_ref().expect("resumed task publishes").analysis();
    let beat = analysis.beat().expect("resumed beat slot");
    assert!(analysis.is_settled());
    assert_eq!(beat.state(), BeatState::Final);
    assert_eq!(beat.unanalysed(), &[0..ORIGIN]);
    assert!(
        completed_beats
            .iter()
            .all(|saved| beat.artifact().beats().contains(saved)),
        "completed nonzero range marks survive archive and refill",
    );
}
