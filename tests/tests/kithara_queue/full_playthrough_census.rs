#![cfg(not(target_arch = "wasm32"))]

//! A three-track queue played from the first frame of the first track to the
//! last frame of the last, with every output frame attributed to the track
//! that produced it.
//!
//! `PlayerTrack::render` names the track, the block-relative span it was asked
//! for, and the track's own media clock. What a track *actually* contributed
//! to a block is the clock's increase across it, not the span it was handed,
//! so the census reads a per-track active window on the session axis. Two
//! properties follow, and together they are the two halves of a premature
//! switch: a track must stay active for its whole length, and two tracks may
//! share output frames only inside a crossfade the queue announced.
//!
//! The rendered audio then answers the same question twice more without the
//! probe: its ramp direction says which track each stretch came from, and
//! Cochlea says the take never falls silent for longer than the handover's
//! block quantum and never sums two tracks above the level one plays at.

use std::{collections::BTreeMap, path::Path};

use kithara::{
    events::{AdvanceReason, Event, QueueEvent, TrackId},
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig, ResourceSrc, player::PlayerControl},
    queue::{Queue, QueueConfig, QueueControl, Transition, test_utils::QueueProbe},
};
use kithara_integration_tests::{
    HlsFixtureBuilder, TestServerHelper, TestTempDir,
    cochlea::CochleaReport,
    fixture_protocol::PcmPattern,
    offline::{OfflinePlayerHarness, OfflinePlayerOptions},
    temp_dir,
};
use kithara_test_fixtures::signal::{FrameClass, classify_windows};
use kithara_test_utils::probe::{IntoProbeArg, capture as probe_capture, capture::Recorder};

use crate::bufpool_ext::TestPools;

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BLOCK_FRAMES: usize = 512;
const SEGMENTS: usize = 3;
const SEGMENT_SECS: f64 = 2.0;
/// Length the fixture is built to. Each track's own reported duration drives
/// the census; this only catches a fixture that stopped resembling itself.
const NOMINAL_TRACK_SECS: f64 = 6.0;
const NOMINAL_TRACK_TOLERANCE_SECS: f64 = 1.0;
const CROSSFADE_SECS: f32 = 1.0;
/// Three tracks plus slack; the loop leaves early on `QueueEnded`.
const BLOCK_BUDGET: usize = 3_000;
/// Provenance classification window and its tolerance, as used by the
/// neighbouring boundary tests.
const CLASS_WINDOW: usize = 64;
const CLASS_TOL: f32 = 0.5;
/// Windows a class must hold to count as a track rather than a seam artefact:
/// a quarter second, against tracks that run for seconds.
const SUSTAINED_WINDOWS: usize = 172;
/// Level the offline session renders at, below the limiter's knee.
const CENSUS_LEVEL: f32 = 0.5;
/// Amplitude a sample counts as silence below, as the seam tests next door
/// read it. The fixture's ramp crosses zero under it for a few hundred
/// microseconds, which is two orders below a render block.
const SILENCE_THRESHOLD: f32 = 1.0e-3;
/// How far the take's peak may sit from the level one track plays at. A fade
/// law that attenuates keeps the sum under a single track's own peak; one that
/// does not doubles it, which is 6 dB away.
const PEAK_BAND_DB: f64 = 0.5;

/// What separates one track from the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seam {
    /// No overlap: the successor starts where the predecessor ended.
    Gapless,
    /// A `CROSSFADE_SECS` overlap the queue announces before it begins.
    Crossfade,
}

impl Seam {
    const fn crossfade_seconds(self) -> f32 {
        match self {
            Self::Gapless => 0.0,
            Self::Crossfade => CROSSFADE_SECS,
        }
    }

    const fn transition(self) -> Transition {
        match self {
            Self::Gapless => Transition::None,
            Self::Crossfade => Transition::Crossfade,
        }
    }
}

fn frames_from_secs(secs: f64) -> i64 {
    let frames = secs * f64::from(SAMPLE_RATE);
    num_traits::cast(frames).expect("a fixture duration fits the session axis")
}

async fn hls_flac_resource(
    player: &PlayerControl<TestPools>,
    server: &TestServerHelper,
    cache_dir: &Path,
    pattern: PcmPattern,
) -> Resource {
    let created = server
        .create_hls(
            HlsFixtureBuilder::new()
                .variant_count(1)
                .segments_per_variant(SEGMENTS)
                .segment_duration_secs(SEGMENT_SECS)
                .packaged_audio_per_variant_pcm_flac(SAMPLE_RATE, CHANNELS, vec![pattern]),
        )
        .await
        .expect("create census HLS fixture");
    let config = ResourceConfig::<TestPools>::for_src(
        ResourceSrc::parse(created.master_url().as_str()).expect("valid HLS master URL"),
    )
    .store(kithara_integration_tests::disk_asset_store(cache_dir))
    .build();
    let config = player
        .prepare_config(config)
        .expect("prepare census HLS resource");
    let mut resource = Resource::new(config)
        .await
        .expect("open census HLS resource");
    let _ = resource.preload().await;
    resource
}

struct Census {
    harness: OfflinePlayerHarness,
    queue: QueueControl<TestPools>,
    tracks: Vec<TrackId>,
}

async fn build_queue(
    server: &TestServerHelper,
    temp_dir: &TestTempDir,
    seam: Seam,
    patterns: &[PcmPattern],
) -> Census {
    let harness = OfflinePlayerHarness::with_sample_rate(
        OfflinePlayerOptions::builder()
            .crossfade_duration(seam.crossfade_seconds())
            .block_on_underrun(true)
            .build(),
        SAMPLE_RATE,
    );
    harness.set_host_level(CENSUS_LEVEL);
    let mut config = QueueConfig::builder().player(harness.take_player()).build();
    config.should_autoplay = false;
    let queue: QueueControl<TestPools> = harness.insert_control(Queue::new(config));

    let mut tracks = Vec::with_capacity(patterns.len());
    for (index, pattern) in patterns.iter().enumerate() {
        let resource = hls_flac_resource(
            harness.player(),
            server,
            &temp_dir.path().join(format!("track{index}")),
            *pattern,
        )
        .await;
        tracks.push(queue.insert_loaded_for_test(resource));
    }
    queue
        .select(tracks[0], seam.transition())
        .expect("select the first track");

    Census {
        harness,
        queue,
        tracks,
    }
}

/// Pace each block so the decode worker runs between them; without the yield
/// the worker never refills the ring and the queue starves mid-track.
fn render_block_duration() -> Duration {
    if cfg!(feature = "flash") {
        let frames = u32::try_from(BLOCK_FRAMES).expect("render block size fits u32");
        Duration::from_secs_f64(f64::from(frames) / f64::from(SAMPLE_RATE))
    } else {
        Duration::from_millis(1)
    }
}

#[derive(Default)]
struct QueueLog {
    advances: Vec<(Option<TrackId>, AdvanceReason)>,
    crossfades: usize,
    ended: bool,
    /// Reported duration of each queue position, read while it is current.
    durations: BTreeMap<usize, f64>,
}

#[kithara::flash(true)]
async fn play_to_the_end(census: &Census) -> (Vec<f32>, QueueLog) {
    let block_duration = render_block_duration();
    let mut receiver = census.queue.subscribe();
    let mut log = QueueLog::default();
    let mut rendered = Vec::new();

    for _ in 0..BLOCK_BUDGET {
        let _ = census.queue.tick();
        rendered.extend(census.harness.render(BLOCK_FRAMES));

        if let (Some(index), Some(duration)) = (
            census.queue.current_index(),
            census.queue.duration_seconds(),
        ) && duration > 0.0
        {
            log.durations.insert(index, duration);
        }

        while let Ok(envelope) = receiver.try_recv() {
            match envelope.event {
                Event::Queue(QueueEvent::CurrentTrackAdvance { id, reason }) => {
                    log.advances.push((id, reason));
                }
                Event::Queue(QueueEvent::CrossfadeStarted { .. }) => log.crossfades += 1,
                Event::Queue(QueueEvent::QueueEnded) => log.ended = true,
                _ => {}
            }
        }

        time::sleep(block_duration).await;

        if log.ended {
            break;
        }
    }

    (rendered, log)
}

/// One firing of the render probe: which track was asked for which block, and
/// how much of that track had been served when it was asked.
#[derive(Clone, Copy, Debug)]
struct Firing {
    track: u64,
    block: i64,
    served: u64,
}

fn firings(recorder: &Recorder) -> Vec<Firing> {
    let mut firings: Vec<Firing> = recorder
        .events_with_probe("render")
        .iter()
        .filter_map(|event| {
            let base = event.u64("output_base")?;
            if base == u64::MAX {
                return None;
            }
            let range_start: i64 = i64::from_probe_arg(event.u64("range_start")?);
            Some(Firing {
                track: event.u64("track_id")?,
                block: i64::from_probe_arg(base) + range_start,
                served: event.u64("served_media_frames")?,
            })
        })
        .collect();
    firings.sort_by_key(|firing| (firing.track, firing.block));
    firings
}

/// The session-axis span over which one track actually produced audio.
#[derive(Clone, Copy, Debug)]
struct Active {
    first: i64,
    last: i64,
    served: u64,
}

/// A track is active across a block when its media clock advanced over it, so
/// a block it was asked for but answered with EOF never enters the window.
fn active_windows(firings: &[Firing]) -> BTreeMap<u64, Active> {
    let mut windows: BTreeMap<u64, Active> = BTreeMap::new();
    for pair in firings.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        if before.track != after.track || after.served <= before.served {
            continue;
        }
        windows
            .entry(before.track)
            .and_modify(|window| {
                window.last = after.block;
                window.served = after.served;
            })
            .or_insert(Active {
                first: before.block,
                last: after.block,
                served: after.served,
            });
    }
    windows
}

fn left_channel(rendered: &[f32]) -> Vec<f32> {
    rendered
        .chunks_exact(usize::from(CHANNELS))
        .map(|frame| frame[0])
        .collect()
}

/// Runs of one classification, ignoring the `Unknown` windows a crossfade's
/// mixed span produces.
fn class_runs(rendered: &[f32]) -> Vec<(FrameClass, usize)> {
    let mut runs: Vec<(FrameClass, usize)> = Vec::new();
    for class in classify_windows(&left_channel(rendered), CLASS_WINDOW, CLASS_TOL) {
        if matches!(class, FrameClass::Unknown) {
            continue;
        }
        match runs.last_mut() {
            Some((last, count)) if *last == class => *count += 1,
            _ => runs.push((class, 1)),
        }
    }
    runs
}

/// The take the census can speak for: everything up to the last block the last
/// track was active over. The render loop runs a couple of blocks past
/// `QueueEnded`, and that tail belongs to the loop, not to the queue.
fn played_samples(ordered: &[(u64, Active)]) -> usize {
    let last = ordered
        .last()
        .expect("the census names at least one track")
        .1
        .last;
    usize::try_from(last).expect("the session axis stays positive") * usize::from(CHANNELS)
}

/// Longest run of consecutive silent samples.
fn longest_silence(channel: &[f32]) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for sample in channel {
        if sample.abs() < SILENCE_THRESHOLD {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

async fn run_census(seam: Seam, temp_dir: &TestTempDir) {
    let recorder = probe_capture::install();
    let server = TestServerHelper::new().await;
    let patterns = [
        PcmPattern::Ascending,
        PcmPattern::Descending,
        PcmPattern::Ascending,
    ];
    let census = build_queue(&server, temp_dir, seam, &patterns).await;
    let (rendered, log) = play_to_the_end(&census).await;

    assert!(
        log.ended,
        "the queue must reach its end within {BLOCK_BUDGET} blocks; \
         rendered {} frames",
        rendered.len() / usize::from(CHANNELS)
    );

    let windows = active_windows(&firings(&recorder));
    let expected: Vec<u64> = census.tracks.iter().map(|id| id.as_u64()).collect();
    let mut ordered: Vec<(u64, Active)> = windows.iter().map(|(id, w)| (*id, *w)).collect();
    ordered.sort_by_key(|(_, window)| window.first);

    assert_eq!(
        ordered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        expected,
        "every queued track must produce audio, in queue order"
    );

    let block = i64::try_from(BLOCK_FRAMES).expect("block size fits the session axis");
    let slack = block * 4;
    let lengths: Vec<i64> = (0..expected.len())
        .map(|index| {
            let secs = *log
                .durations
                .get(&index)
                .unwrap_or_else(|| panic!("queue position {index} must report a duration"));
            assert!(
                (secs - NOMINAL_TRACK_SECS).abs() <= NOMINAL_TRACK_TOLERANCE_SECS,
                "the fixture at queue position {index} must be about \
                 {NOMINAL_TRACK_SECS} s long, not {secs} s"
            );
            frames_from_secs(secs)
        })
        .collect();

    for ((id, window), track_frames) in ordered.iter().zip(&lengths) {
        let served = i64::from_probe_arg(window.served);
        assert!(
            (served - track_frames).abs() <= slack,
            "track {id} must serve its whole length: served={served} frames, \
             expected {track_frames} +/- {slack}"
        );
        let span = window.last - window.first;
        assert!(
            (span - track_frames).abs() <= slack,
            "track {id} must stay active for its whole length: span={span} frames, \
             expected {track_frames} +/- {slack}"
        );
    }

    let expected_overlap = frames_from_secs(f64::from(seam.crossfade_seconds()));
    for pair in ordered.windows(2) {
        let ((left_id, left), (right_id, right)) = (pair[0], pair[1]);
        let overlap = left.last - right.first;
        assert!(
            (overlap - expected_overlap).abs() <= slack,
            "tracks {left_id} and {right_id} must overlap by exactly the \
             configured crossfade: overlap={overlap} frames, expected \
             {expected_overlap} +/- {slack}"
        );
        assert!(
            right.first - left.last <= block,
            "no output frame may go unclaimed between tracks {left_id} and \
             {right_id}: the handover lands on the render-block grid, so one \
             block is the whole budget; gap={} frames",
            right.first - left.last
        );
    }

    let runs = class_runs(&rendered);
    let classes: Vec<FrameClass> = runs
        .iter()
        .filter(|(_, count)| *count >= SUSTAINED_WINDOWS)
        .map(|(class, _)| *class)
        .collect();
    assert_eq!(
        classes,
        vec![
            FrameClass::Ascending,
            FrameClass::Descending,
            FrameClass::Ascending
        ],
        "the rendered audio must carry each track's own signal, in queue order; \
         runs={runs:?}"
    );

    let played = &rendered[..played_samples(&ordered)];

    let silence = longest_silence(&left_channel(played));
    assert!(
        silence <= BLOCK_FRAMES,
        "the playthrough may fall silent only for as long as the handover is \
         quantised to: run={silence} frames, one render block is {BLOCK_FRAMES}"
    );

    let report = CochleaReport::measure(played, CHANNELS, SAMPLE_RATE);
    let track_peak_dbfs = 20.0 * f64::from(CENSUS_LEVEL).log10();
    let peak = report
        .sample_peak_dbfs
        .expect("a played take carries a sample peak");
    assert!(
        peak <= track_peak_dbfs + PEAK_BAND_DB,
        "a crossfade must fade the two tracks against each other rather than sum \
         them: peak={peak} dBFS, one track plays at {track_peak_dbfs} dBFS"
    );
    assert_eq!(
        report.leading_silence_ms, 0.0,
        "the queue must start on the first track's first frame: {report:?}"
    );

    let landed: Vec<u64> = log
        .advances
        .iter()
        .filter_map(|(id, _)| id.map(TrackId::as_u64))
        .collect();
    assert_eq!(
        landed,
        expected[1..],
        "the queue must advance into each successor once, in order; \
         advances={:?}",
        log.advances
    );
    assert!(
        log.advances.iter().all(|(_, reason)| matches!(
            reason,
            AdvanceReason::NaturalEof | AdvanceReason::CrossfadePreArm
        )),
        "a full playthrough may only advance at a track boundary — the \
         handover trigger and end-of-track race for it, so either reason is \
         the boundary's; got {:?}",
        log.advances
    );
    assert_eq!(
        log.crossfades,
        match seam {
            Seam::Gapless => 0,
            Seam::Crossfade => expected.len() - 1,
        },
        "a crossfade must be announced exactly at the configured boundaries"
    );
}

/// Gapless: no output frame may be claimed by two tracks at once. A premature
/// switch shows up here as an overlap the configuration never asked for.
#[kithara::test(
    native,
    tokio,
    timeout(Duration::from_secs(180)),
    hang_timeout_secs(20)
)]
async fn gapless_queue_plays_every_track_end_to_end(temp_dir: TestTempDir) {
    run_census(Seam::Gapless, &temp_dir).await;
}

/// Crossfade: the overlap must be exactly the configured one, at the boundary
/// and nowhere else.
#[kithara::test(
    native,
    tokio,
    timeout(Duration::from_secs(180)),
    hang_timeout_secs(20)
)]
async fn crossfaded_queue_plays_every_track_end_to_end(temp_dir: TestTempDir) {
    run_census(Seam::Crossfade, &temp_dir).await;
}
