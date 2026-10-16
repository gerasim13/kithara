#![cfg(not(target_os = "android"))]

use std::{collections::BTreeMap, ops::Range};

use kithara::{
    events::TrackId,
    link::LinkedSnapshot,
    play::{CrossfadeCurve, CrossfadeSettings, DeckMixSettings, DeckMixerConfig, TrackSnapshot},
    queue::QueueSettings,
    signal::{AudioChunkInfo, SegmentId, SourceSpan},
    warp::{Beat, BeatGrid, BeatGridQuery, BeatGridSnapshot, MapPoint, MapPosition},
};
use kithara_command::Seq;
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    offline::linked::{LinkCapture, LinkVerdict},
};
use kithara_render::bridge::{Fade, FadeDir, Slot, SlotSnapshot, SlotState};
use kithara_test_utils::tracing;

use super::{
    consumed::{Capture, SourceIdentity},
    control::{CONTROL_GAIN, DecodedCapture, DecodedObserver},
    imports::*,
    providers::Provider,
};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;
const SOURCE_BLOCK: usize = 8192;
const SOURCES: &[&str] = &[
    "rhythm_wav_house_124_aligned",
    "rhythm_wav_downtempo_96_aligned",
    "rhythm_wav_techno_132_aligned",
    "rhythm_wav_trip_hop_74_aligned",
    "rhythm_wav_breakbeat_140_aligned",
];

type NativeStart = (TrackId, Slot, SessionFrame, Fade);

struct Voice {
    item: TrackId,
    model: Arc<BeatGridModel>,
    decoded: Arc<Mutex<DecodedCapture>>,
    fixture: kithara_integration_tests::pcm_oracle::Audio,
    first: BTreeMap<SegmentId, SlotSnapshot>,
    bound: Option<(SlotSnapshot, BeatGridSnapshot, f32)>,
    checks: usize,
}

struct Interval {
    voice: usize,
    slot: Slot,
    start: i64,
    active: Range<i64>,
    identity: SourceIdentity,
    source: SourceSpan,
    physical_origin: u64,
    physical_basis: SourceSpan,
    physical_domain: Range<u64>,
}

fn frame(at: SessionFrame) -> i64 {
    i64::from(at)
}

fn beat_frame(grid: &BeatGridSnapshot, beat: f64) -> i64 {
    let query = grid.position_at(MapPoint::new(
        grid.stamp(),
        Beat::new(beat).expect("finite Host beat"),
    ));
    let BeatGridQuery::Resolved(position) = query else {
        panic!("Host inverse query at beat {beat}: {query:?}; actual grid {grid:?}");
    };
    let MapPosition::Session(at) = *position.value().value() else {
        panic!("Host beat uses the session axis");
    };
    frame(at)
}

fn host_beat(grid: &BeatGridSnapshot, at: i64) -> f64 {
    let query = grid.beat_at(MapPoint::new(
        grid.stamp(),
        MapPosition::Session(SessionFrame::new(at)),
    ));
    let BeatGridQuery::Resolved(beat) = query else {
        panic!("Host beat query at actual session frame {at}: {query:?}; actual grid {grid:?}");
    };
    f64::from(*beat.value().value())
}

fn envelope(capture: &LinkCapture, item: TrackId, session: i64, mixer: DeckMixerConfig) -> f32 {
    let (_, _, start, fade) = *capture
        .starts
        .iter()
        .find(|(owner, ..)| *owner == item)
        .expect("every audible item has its own native Start");
    let settings = match fade {
        Fade::Crossfade(settings) => settings,
        Fade::Declick => CrossfadeSettings {
            duration: mixer.declick().smooth_seconds.max(0.0),
            curve: CrossfadeCurve::Linear,
            depth: 0.0,
            position: 0.5,
        },
    };
    let gain = |settings: CrossfadeSettings, at: SessionFrame, incoming: bool| {
        let rate: f32 = RATE.as_();
        let frames: u64 = (settings.duration * rate).round().as_();
        let elapsed =
            u64::try_from(session - frame(at)).expect("gain lies after its native command");
        let progress = if frames <= 1 {
            1.0
        } else {
            let elapsed: f32 = elapsed.as_();
            let last: f32 = (frames - 1).as_();
            (elapsed / last).min(1.0)
        };
        let (out, into) = settings.gains(progress);
        if incoming { into } else { out }
    };
    if let Some((_, _, at, settings, FadeDir::Out)) =
        capture.fades.iter().find(|(owner, _, at, _, dir)| {
            *owner == item && *dir == FadeDir::Out && frame(*at) <= session
        })
    {
        gain(*settings, *at, false)
    } else {
        gain(settings, start, true)
    }
}

fn intervals(voices: &[Voice], capture: &LinkCapture) -> Vec<Interval> {
    let mut result: Vec<_> = voices
        .iter()
        .enumerate()
        .flat_map(|(index, voice)| voice_intervals(index, voice, capture))
        .collect();
    result.sort_by_key(|interval| (interval.slot.get(), interval.active.start));
    result
}

fn first_strong_beat(voice: &Voice) -> Duration {
    let raw = voice.model.as_raw();
    assert!(
        raw.meter.is_some(),
        "fixed musical fixtures declare their meter"
    );
    Duration::try_from_secs_f64(raw.downbeats.first().expect("declared first downbeat").at)
        .expect("finite declared first downbeat")
}

fn residency_origin(voice: &Voice, info: AudioChunkInfo) -> u64 {
    let events = kithara_test_utils::test::usdt::events_of("lane_source_ready");
    let mut ready = events.iter().filter(|event| {
        event.field("item") == Some(u64::from(voice.item))
            && event.field("segment") == Some(info.segment.get())
            && event.field("source_revision") == Some(info.source_revision)
    });
    let event = ready.next().unwrap_or_else(|| {
        panic!(
            "UNSUPPORTED_ORACLE_WITNESS: item {:?}, segment {:?}: no actual current-ready source basis",
            voice.item, info.segment
        )
    });
    assert!(
        ready.next().is_none(),
        "UNSUPPORTED_ORACLE_WITNESS: current-ready source basis must be unique per item/segment/revision"
    );
    event.field("lane_seq").expect("actual preparation receipt");
    event
        .field("source_first_frame")
        .expect("actual first pre-Warp physical input")
}

fn native_end(capture: &LinkCapture, item: TrackId) -> i64 {
    capture
        .faded
        .iter()
        .chain(&capture.ended)
        .filter(|(owner, ..)| *owner == item)
        .map(|(_, _, at)| frame(*at))
        .min()
        .expect("native terminal frame")
}

fn voice_intervals(index: usize, voice: &Voice, capture: &LinkCapture) -> Vec<Interval> {
    let mut result = Vec::new();
    let &(_, slot, start, _) = capture
        .starts
        .iter()
        .find(|(owner, ..)| *owner == voice.item)
        .expect("one actual Start per item");
    let end = native_end(capture, voice.item);
    assert!(end > frame(start), "each real source has an audible domain");
    let packets = source_packets(voice, capture, slot, start, end);
    let (&first_start, &first_info) = packets
        .first_key_value()
        .expect("first actual complete source packet");
    assert_eq!(
        first_start,
        frame(start),
        "first actual source packet owns the native Start"
    );
    assert_eq!(
        first_info.lane_frame, 0,
        "first actual packet starts at the native segment origin"
    );
    let first_span = first_info
        .source_span
        .expect("first retained source geometry");
    assert_eq!(
        first_span.output_frames(),
        u64::from(first_info.frames),
        "first original packet retains its complete output extent"
    );
    assert_eq!(
        first_span.position_at(0),
        Some(first_strong_beat(voice)),
        "actual first SourceSpan starts at the supplied first strong beat"
    );
    assert_eq!(
        (first_info.spec.sample_rate.get(), first_info.spec.channels),
        (RATE, 2),
        "UNSUPPORTED_ORACLE_WITNESS: the fixed native source format never changes"
    );
    let identity = SourceIdentity::from(first_info);
    for info in packets.values() {
        assert_eq!(
            (
                info.segment,
                info.spec,
                info.source_revision,
                info.media_origin
            ),
            (
                first_info.segment,
                identity.spec,
                identity.source_revision,
                identity.media_origin,
            ),
            "UNSUPPORTED_ORACLE_WITNESS: no source Segment/spec/revision/origin change after actual Start"
        );
    }
    let initial_raw = first_span
        .source_ratio_at(0)
        .expect("first actual source span origin");
    let (initial_origin, initial_basis) = voice
        .decoded
        .lock()
        .physical_basis(identity, initial_raw)
        .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
    let origin = residency_origin(voice, first_info);
    let initial_physical = initial_basis
        .output_ratio_at_source(initial_origin, initial_raw)
        .expect("first actual decoded physical source origin");
    assert!(
        initial_physical.0 >= u128::from(origin) * initial_physical.1.get(),
        "actual cue never precedes the witnessed retained prefix"
    );
    let endpoint = initial_basis
        .output_ratio_at_source(
            initial_origin,
            first_span
                .source_ratio_at(first_span.output_frames())
                .expect("first exact source-span endpoint"),
        )
        .expect("first physical source-window endpoint");
    let demand = endpoint.0.div_ceil(endpoint.1.get()) + 17 - u128::from(origin);
    assert!(
        demand <= (2 * SOURCE_BLOCK) as u128,
        "UNSUPPORTED_ORACLE_WITNESS: first no-keylock quantum exceeds the guaranteed residency capacity"
    );
    let physical_domain = voice
        .decoded
        .lock()
        .fixture_domain(first_info, &voice.fixture, origin)
        .unwrap_or_else(|error| panic!("named source {:?}: {error}", voice.item));
    let consumed = Capture::for_track(u64::from(voice.item), RATE)
        .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
    let slices = consumed
        .covered(frame(start)..end)
        .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
    let mut frontier = frame(start);
    for (slice_start, interval) in slices {
        assert_eq!(
            (
                interval.identity.spec,
                interval.identity.source_revision,
                interval.identity.media_origin,
            ),
            (
                identity.spec,
                identity.source_revision,
                identity.media_origin
            ),
            "every actual consumed slice retains the first native source identity"
        );
        let span = interval.source;
        let slice_end = slice_start + i64::try_from(span.output_frames()).expect("slice frames");
        let active = slice_start.max(frame(start))..slice_end.min(end);
        assert_eq!(
            active.start, frontier,
            "item {:?}: actual consumed SourceSpan has a gap or overlap at {frontier}",
            voice.item
        );
        frontier = active.end;
        let raw = span
            .source_ratio_at(u64::try_from(active.start - slice_start).expect("slice offset"))
            .expect("exact consumed interval has a raw source coordinate");
        let (physical_origin, physical_basis) = voice
            .decoded
            .lock()
            .physical_basis(interval.identity, raw)
            .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
        result.push(Interval {
            voice: index,
            slot,
            start: slice_start,
            active,
            identity: interval.identity,
            source: span,
            physical_origin,
            physical_basis,
            physical_domain: physical_domain.clone(),
        });
    }
    assert_eq!(
        frontier, end,
        "every audible sample through the native end has actual source geometry"
    );
    result
}

fn source_packets(
    voice: &Voice,
    capture: &LinkCapture,
    slot: Slot,
    start: SessionFrame,
    end: i64,
) -> BTreeMap<i64, AudioChunkInfo> {
    let mut packets = BTreeMap::new();
    for &(at, owner, seat, consumed) in &capture.consumed {
        if owner != voice.item {
            continue;
        }
        assert_eq!(
            seat, slot,
            "item never migrates away from its allocated slot"
        );
        let Some(info) = consumed.source else {
            continue;
        };
        let Some(mark) = consumed.mark else {
            continue;
        };
        assert_eq!(
            mark.lane.segment, info.segment,
            "actual packet/mark segment"
        );
        if mark.lane.frame <= info.lane_frame && info.frames > 0 {
            continue;
        }
        let custody = capture
            .segments
            .iter()
            .filter(|(item, seat, applied, segment, _)| {
                *item == owner && *seat == slot && *applied <= at && *segment == info.segment.get()
            })
            .map(|(_, _, applied, _, _)| frame(*applied))
            .max()
            .unwrap_or_else(|| {
                panic!(
                    "native Attach/Adopt owns every consumed source segment; \
                     owner={owner:?}; slot={slot:?}; observed_at={at:?}; \
                     snapshot={consumed:?}; start={start:?}; end={end}; \
                     segments={:?}; starts={:?}",
                    capture.segments, capture.starts,
                )
            });
        let origin = custody.max(frame(start));
        if consumed.state == SlotState::Playing
            && frame(mark.session) > frame(start)
            && frame(mark.session) < end
        {
            assert_eq!(
                frame(mark.session) - i64::try_from(mark.lane.frame).expect("lane frame"),
                origin,
                "native start/adoption owns the active lane/session translation"
            );
        }
        if info.frames == 0 {
            assert!(
                info.end_of_track,
                "only the actual terminal packet has no frames"
            );
            continue;
        }
        let span = info
            .source_span
            .expect("actual audio packet has an exact SourceSpan");
        assert!(span.output_frames() > 0);
        let packet_start =
            origin + i64::try_from(info.lane_frame).expect("packet lane fits session");
        if let Some(previous) = packets.insert(packet_start, info) {
            assert_eq!(
                previous, info,
                "same source interval has one immutable geometry"
            );
        }
    }
    packets
}

fn reference(
    voices: &[Voice],
    capture: &LinkCapture,
    intervals: &[Interval],
    mixer: DeckMixerConfig,
    domain: Range<i64>,
    omit: Option<usize>,
    shift: Option<usize>,
) -> Vec<f32> {
    let frames = usize::try_from(domain.end - domain.start).expect("reference domain");
    let wall = Instant::now();
    let mut reported = wall;
    let mut source_seen = None;
    let mut expected = vec![0.0_f32; frames * 2];
    for interval in intervals {
        if omit == Some(interval.voice) {
            continue;
        }
        let active = interval.active.start.max(domain.start)..interval.active.end.min(domain.end);
        if active.is_empty() {
            continue;
        }
        let voice = &voices[interval.voice];
        if source_seen != Some(interval.voice) || reported.elapsed() >= Duration::from_secs(1) {
            tracing::info!(target: "kithara_test::sync::queue321", phase = "reference_progress",
                wall_s = wall.elapsed().as_secs_f64(), item = u64::from(voice.item),
                session_frame = active.start, domain_start = domain.start, domain_end = domain.end,
                omit = ?omit, shift = ?shift);
            source_seen = Some(interval.voice);
            reported = Instant::now();
        }
        let decoded = voice.decoded.lock();
        let span = interval.source;
        for session in active {
            let offset = u64::try_from(session - interval.start).expect("source offset");
            let raw = span
                .source_ratio_at(offset)
                .expect("physical source coordinate");
            let next = span
                .source_ratio_at(offset + 1)
                .expect("physical source endpoint");
            let physical = interval
                .physical_basis
                .output_ratio_at_source(interval.physical_origin, raw)
                .expect("actual decoded physical coordinate");
            let endpoint = interval
                .physical_basis
                .output_ratio_at_source(interval.physical_origin, next)
                .expect("actual decoded physical endpoint");
            let speed = super::control::physical_source_speed(physical, endpoint)
                .expect("actual source speed");
            let position = if shift == Some(interval.voice) {
                (
                    physical
                        .0
                        .checked_add(physical.1.get())
                        .expect("negative control source shift"),
                    physical.1,
                )
            } else {
                physical
            };
            let gain = envelope(capture, voice.item, session, mixer);
            let base = usize::try_from(session - domain.start).expect("reference offset") * 2;
            for channel in 0..2 {
                let sample = decoded
                    .sample_at_domain(
                        interval.identity,
                        position,
                        speed,
                        channel,
                        &interval.physical_domain,
                    )
                    .unwrap_or_else(|error| {
                        panic!("source {:?} session {session}: {error}", voice.item)
                    });
                if gain == 1.0 {
                    expected[base + channel] += sample;
                } else if gain != 0.0 {
                    expected[base + channel] = sample.mul_add(gain, expected[base + channel]);
                }
            }
        }
        drop(decoded);
    }
    let ceiling = kithara::effects::LimiterConfig::default().ceiling();
    for sample in &mut expected {
        *sample *= CONTROL_GAIN;
        assert!(
            sample.is_finite() && sample.abs() < ceiling,
            "declared mix must retain limiter headroom"
        );
    }
    expected
}

struct QueueMusic {
    voices: Vec<Voice>,
    deck: LinkedQueueHandle<TestPools>,
    master_wav: Option<AudioArtifactTap>,
    output_wav: Option<AudioArtifactTap>,
    master: TapProbe,
    host: OfflineHostHarness<TestPools, LinkedOwner<TestPools>>,
    mixer: DeckMixerConfig,
    _worker: PlayWorker<TestPools>,
}

struct BoundaryCopy {
    item: TrackId,
    seat: Slot,
    before: SlotSnapshot,
    after: (LinkedSnapshot<TrackSnapshot>, SlotSnapshot),
}

#[derive(Default)]
struct CapturedMusic {
    failures: Vec<String>,
    answered: Vec<(TrackId, Seq)>,
    pcm: Vec<f32>,
    retarget: Option<(SessionFrame, [TrackId; 2])>,
    checks: Vec<serde_json::Value>,
    selected_items: Vec<TrackId>,
    primary_beats: Vec<(i64, TrackId)>,
    boundary_copy: Option<BoundaryCopy>,
    output_delta: f32,
}

impl QueueMusic {
    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn new() -> Self {
        let sample_rate = NonZeroU32::new(RATE).expect("source rate");
        let pools = pools();
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
        let initial = Tempo::new(100.0).expect("initial Host tempo");
        let trajectory = TempoTrajectory::new(
            TempoStep {
                frame: SessionFrame::new(0),
                beat: SessionBeat::default(),
                tempo: initial,
            },
            std::num::NonZeroU16::new(4).expect("meter"),
            sample_rate,
        );
        let axis = trajectory.clone();
        let config = HostConfig::offline(pools)
            .max_block_frames(
                NonZeroU32::new(BLOCK.try_into().expect("block frames fit u32")).expect("block"),
            )
            .settings(
                HostSettings::builder()
                    .sample_rate(sample_rate)
                    .tempo(initial)
                    .metronome(MetronomeConfig::builder().level(0.25).duck(0.25).build())
                    .build(),
            )
            .build();
        let host: OfflineHostHarness<TestPools, LinkedOwner<TestPools>> =
            OfflineHostHarness::layered(config, None, move |core| {
                LinkedHost::new(
                    LinkedHostConfig::builder()
                        .inner(core)
                        .trajectory(axis)
                        .build(),
                )
            })
            .await
            .expect("real offline Host");
        host.with(|host| host.metronome().set_enabled(true))
            .await
            .expect("Host metronome");
        let master = host
            .attach_tap(Tap::Master, BLOCK * 2 * 4)
            .await
            .expect("actual Master tap");
        let label = artifact_label();
        let output_wav = AudioArtifactTap::from_env(
            &format!("{label}-queue321-five-music-tracks-100-to-120-output"),
            RATE,
            2,
        )
        .expect("Output WAV export");
        let master_wav = AudioArtifactTap::from_env(
            &format!("{label}-queue321-five-music-tracks-100-to-120-master"),
            RATE,
            2,
        )
        .expect("Master WAV export");
        let mixer = DeckMixerConfig::builder()
            .sample_rate(sample_rate)
            .slots(NonZeroUsize::new(2).expect("two actual reusable mixer slots"))
            .mix(DeckMixSettings::builder().level(CONTROL_GAIN).build())
            .build();
        let crossfade = CrossfadeSettings::default();
        assert_eq!(crossfade.duration, 1.0, "original #321 crossfade");
        let observation = Arc::new(Mutex::new(LinkObservation {
            capture: Some(LinkCapture::default()),
            ..LinkObservation::default()
        }));
        let warp = kithara::warp::WarpConfig::builder()
            .render_quantum_frames(NonZeroUsize::new(BLOCK).expect("one observed Warp quantum"))
            .build();
        let track = kithara::play::TrackSettings::default();
        assert!(
            !track.keylock()
                && track
                    .backend()
                    .capabilities()
                    .contains(kithara::stretch::BackendCapabilities::RATE)
                && warp.region_plan().is_none()
                && warp.source_block_frames().get() == SOURCE_BLOCK,
            "UNSUPPORTED_ORACLE_WITNESS: residency proof requires the fixed native no-keylock RATE chain without a region plan"
        );
        let queue = Queue::new(
            QueueConfig::with_factory(LinkedFactory::new(
                LinkedConfig::builder()
                    .inner(ObservedFactory::new(observation.clone()))
                    .trajectory(trajectory)
                    .build(),
            ))
            .mixer(mixer)
            .settings(QueueSettings::builder().crossfade(crossfade).build())
            .track(track)
            .should_autoplay(false)
            .prep(
                ResourcePrep::builder()
                    .worker(worker.clone())
                    .block_on_underrun(false)
                    .warp(warp)
                    .build(),
            )
            .store(memory_asset_store())
            .build(),
        );
        let deck = host
            .insert_linked(queue, observation)
            .await
            .expect("one ordinary Linked Queue");
        Self {
            host,
            deck,
            master,
            mixer,
            voices: Vec::new(),
            output_wav,
            master_wav,
            _worker: worker,
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn append_sources(&mut self) {
        let target = self.deck.id();
        self.host
            .with(move |host| {
                host.send(LinkedHostCommand::Sync {
                    deck: target,
                    on: true,
                })
            })
            .await
            .expect("SYNC through the actual Host owner");
        let provider = Provider::Rhythm(SOURCES);
        for (index, name) in SOURCES.iter().enumerate() {
            hang_tick!(format!("queue321: append real source {index}: {name}"));
            let asset =
                kithara_test_fixtures::assets::by_name(name).expect("existing musical fixture");
            let path = asset.path().expect("musical fixture materialized on disk");
            let fixture = kithara_integration_tests::pcm_oracle::decode_native(asset.bytes())
                .await
                .expect("independent native PCM of this named musical fixture");
            assert_eq!(
                (
                    fixture.sample_rate,
                    fixture.channels,
                    fixture.samples.len() / 2
                ),
                (RATE, 2, RATE as usize * 12),
                "the fixed fixture is twelve seconds of stereo music"
            );
            let ArtifactSource::Value(model) = provider.beat_grid(index) else {
                panic!("declared fixture analysis");
            };
            let config = ResourceConfig::for_src(
                ResourceSrc::parse(path.to_str().expect("source path")).expect("fixture source"),
            )
            .store(memory_asset_store())
            .discriminator(format!("queue321-{index}"))
            .beat_grid(ArtifactSource::Value(model.clone()))
            .build();
            let decoded = Arc::new(Mutex::new(DecodedCapture::default()));
            let observer = DecodedObserver(decoded.clone());
            let control = self.deck.control().clone();
            let item = self
                .host
                .run(move || {
                    let item = control.append(TrackSource::Config(Box::new(config)))?;
                    control.attach_observer(item, observer);
                    Ok::<_, kithara::queue::QueueError>(item)
                })
                .await
                .expect("append to the same public QueueControl");
            self.voices.push(Voice {
                item,
                model,
                decoded,
                fixture,
                first: BTreeMap::new(),
                bound: None,
                checks: 0,
            });
            hang_reset!();
        }
    }

    #[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
    async fn run(&mut self) -> CapturedMusic {
        let wall = Instant::now();
        let mut reported = wall;
        let mut starts_seen = 0;
        let mut law_time = Duration::ZERO;
        let mut render_time = Duration::ZERO;
        let mut history_visits = 0;
        let mut data = CapturedMusic::default();
        let started = self.start_clock(&mut data).await;
        hang_reset!();
        let first_frames: f64 = (BLOCK - 1).as_();
        time::sleep(
            Duration::from_secs_f64(first_frames / f64::from(RATE))
                .saturating_sub(started.elapsed()),
        )
        .await;
        let source_count: f64 = SOURCES.len().as_();
        let max_frames: u64 = (((source_count * 4.0)
            + self
                .voices
                .iter()
                .map(|voice| 12.0 * voice.model.as_raw().bpm / 100.0)
                .sum::<f64>())
            * f64::from(RATE))
        .ceil()
        .as_();
        let mut grids = Vec::new();
        let mut playing = false;
        let mut postroll = None;
        let mut next_beat = None;
        loop {
            hang_tick!(format!(
                "queue321: real Host frame {}, starts {}, grids {}",
                self.host.position(),
                self.deck
                    .observe(|o| o.capture.as_ref().expect("capture").starts.len()),
                data.answered.len()
            ));
            assert!(
                self.host.position() <= max_frames,
                "five fixed sources must reach a native final Ended"
            );
            let control = self.deck.control().clone();
            let grid = self
                .host
                .with(move |host| {
                    control.tick().expect("normal app Queue tick");
                    host.snapshot()
                })
                .await;
            let now = SessionFrame::new(i64::try_from(self.host.position()).expect("Host frame"));
            self.deck.observe(|o| {
                let capture = o.capture.as_ref().expect("typed capture");
                assert!(
                    capture.faults.is_empty(),
                    "first native playback fault at {now:?}; faults={:?}; consumed={:?}; \
                     tracks={:?}; starts={:?}; segments={:?}; fades={:?}; \
                     consumed_tail={:?}; speeds={:?}; receipts={:?}; deck_events={:?}",
                    capture.faults,
                    o.consumed,
                    o.tracks,
                    capture.starts,
                    capture.segments,
                    capture.fades,
                    capture.consumed.iter().rev().take(8).collect::<Vec<_>>(),
                    o.speeds,
                    o.settled,
                    o.timeline
                        .iter()
                        .filter(|event| event["event"] == "deck event")
                        .collect::<Vec<_>>(),
                );
            });
            grids.push((now, grid.clone()));
            self.deliver_grids(&mut data).await;
            if !playing
                && data
                    .answered
                    .iter()
                    .any(|(item, _)| *item == self.voices[0].item)
            {
                let control = self.deck.control().clone();
                self.host.run(move || control.play()).await;
                playing = true;
            }
            if let Some(current) = self.deck.current()
                && data.selected_items.last() != Some(&current.id)
            {
                data.selected_items.push(current.id);
            }
            let snapshot = self.deck.read_snapshot().await;
            let consumed: Vec<_> = snapshot
                .iter()
                .map(|(track, slot)| (track.track.item, *slot))
                .collect();
            let inspected_before = data.checks.len();
            let law_started = Instant::now();
            self.inspect_bindings(
                &snapshot,
                &grid,
                &grids,
                (now, next_beat == Some(frame(now))),
                None,
                &mut data,
            );
            law_time += law_started.elapsed();
            history_visits += (data.checks.len() - inspected_before) * grids.len();
            self.retarget_overlap(&consumed, now, &mut data).await;
            let starts = self
                .deck
                .observe(|o| o.capture.as_ref().expect("capture").starts.len());
            if starts != starts_seen || reported.elapsed() >= Duration::from_secs(1) {
                self.trace_runtime(
                    wall.elapsed().as_secs_f64(),
                    &data,
                    grids.len(),
                    law_time.as_micros(),
                    render_time.as_micros(),
                    history_visits,
                );
                starts_seen = starts;
                reported = Instant::now();
            }
            let final_end = self.deck.observe(|o| {
                o.capture
                    .as_ref()
                    .expect("capture")
                    .ended
                    .iter()
                    .find(|(item, ..)| *item == self.voices[4].item)
                    .map(|(_, _, at)| *at)
            });
            if let Some(at) = final_end {
                let required = *postroll.get_or_insert_with(|| frame(at) + BLOCK as i64);
                if i64::try_from(self.host.position()).expect("Host frame") >= required {
                    break;
                }
            }
            let (frames, boundary) = pull_frames(&consumed, &grid, now);
            let render_started = Instant::now();
            let started = self.render_block(frames, &mut data).await;
            render_time += render_started.elapsed();
            if next_beat == Some(frame(now))
                && snapshot.iter().any(|(_, slot)| {
                    slot.state == SlotState::Playing
                        && slot.mark.zip(slot.source).is_some_and(|(mark, info)| {
                            mark.lane.frame > 0 && mark.lane.frame == info.lane_frame
                        })
                })
            {
                let copied = self.deck.read_snapshot().await;
                let law_started = Instant::now();
                let inspected_before = data.checks.len();
                self.inspect_bindings(
                    &snapshot,
                    &grid,
                    &grids,
                    (now, true),
                    Some(&copied),
                    &mut data,
                );
                law_time += law_started.elapsed();
                history_visits += (data.checks.len() - inspected_before) * grids.len();
            }
            next_beat = Some(boundary);
            hang_reset!();
            let rendered_frames: f64 = frames.as_();
            time::sleep(
                Duration::from_secs_f64(rendered_frames / f64::from(RATE))
                    .saturating_sub(started.elapsed()),
            )
            .await;
        }
        self.trace_runtime(
            wall.elapsed().as_secs_f64(),
            &data,
            grids.len(),
            law_time.as_micros(),
            render_time.as_micros(),
            history_visits,
        );
        tracing::info!(target: "kithara_test::sync::queue321", phase = "render_complete",
            wall_s = wall.elapsed().as_secs_f64(), frame = self.host.position(), frames = data.pcm.len() / 2);
        data
    }

    fn trace_runtime(
        &self,
        wall_s: f64,
        data: &CapturedMusic,
        grids: usize,
        law_us: u128,
        render_us: u128,
        history_visits: usize,
    ) {
        let current = self
            .deck
            .current()
            .map(|track| (u64::from(track.id), format!("{:?}", track.status)));
        self.deck.observe(|o| {
            let capture = o.capture.as_ref().expect("capture");
            let tracks: Vec<_> = o
                .tracks
                .iter()
                .map(|track| {
                    (
                        u64::from(track.track.item),
                        track.track.slot,
                        format!("{:?}", track.track.status),
                        format!("{:?}", track.sync),
                    )
                })
                .collect();
            let loaded = o
                .dispatcher
                .iter()
                .filter(|(_, verdict)| *verdict == LinkVerdict::Applied)
                .count();
            tracing::info!(target: "kithara_test::sync::queue321", phase = "progress",
                wall_s, frame = self.host.position(), loads = ?o.loads, loaded,
                answered = data.answered.len(), current = ?current, tracks = ?tracks,
                native_starts = ?capture.starts, faded = ?capture.faded, ended = ?capture.ended,
                consumed_points = capture.consumed.len(), grids, law_checks = data.checks.len(),
                law_us, render_us, history_visits);
        });
    }

    async fn start_clock(&mut self, data: &mut CapturedMusic) -> Instant {
        assert_eq!(
            self.host.position(),
            0,
            "startup capture begins at session zero"
        );
        self.deck.observe(|o| {
            assert!(
                o.capture.as_ref().expect("capture").starts.is_empty(),
                "no native source Start precedes the initial Host callback"
            );
        });
        let started = self.render_block(BLOCK - 1, data).await;
        assert_eq!(self.host.position(), (BLOCK - 1) as u64);
        assert_eq!(data.pcm.len(), (BLOCK - 1) * 2);
        assert!(
            data.pcm.iter().all(|sample| *sample == 0.0),
            "the complete startup Master interval is retained silence before Play"
        );
        self.deck.observe(|o| {
            assert!(
                o.capture.as_ref().expect("capture").starts.is_empty(),
                "the initial Host callback runs before any source starts"
            );
        });
        let evidence = serde_json::json!({
            "capture_origin_session_frame": 0,
            "first_callback_end_session_frame": self.host.position(),
            "master_frames_retained": data.pcm.len() / 2,
            "source_starts": 0,
            "master_silence": true,
        });
        for tap in [&mut self.output_wav, &mut self.master_wav]
            .into_iter()
            .flatten()
        {
            tap.evidence("startup", evidence.clone());
        }
        started
    }

    async fn deliver_grids(&self, data: &mut CapturedMusic) {
        let target = self.deck.id();
        let loads = self.deck.observe(|observation| {
            observation
                .loads
                .iter()
                .copied()
                .filter(|(_, seq)| {
                    observation
                        .dispatcher
                        .contains(&(*seq, LinkVerdict::Applied))
                })
                .collect::<Vec<_>>()
        });
        for (item, load) in loads {
            if data.answered.contains(&(item, load)) {
                continue;
            }
            let model = self
                .voices
                .iter()
                .find(|voice| voice.item == item)
                .expect("one of five sources")
                .model
                .clone();
            self.host
                .with(move |host| {
                    host.send(LinkedHostCommand::Grid {
                        deck: target,
                        answer: GridAnswer {
                            item,
                            load,
                            model: Ok((*model).clone()),
                        },
                    })
                })
                .await
                .expect("item and actual Load scoped grid");
            data.answered.push((item, load));
        }
    }

    async fn retarget_overlap(
        &mut self,
        consumed: &[(TrackId, SlotSnapshot)],
        now: SessionFrame,
        data: &mut CapturedMusic,
    ) {
        if data.retarget.is_none()
            && self
                .deck
                .observe(|o| o.capture.as_ref().expect("capture").starts.len() == 3)
        {
            let overlap: Vec<_> = consumed
                .iter()
                .filter(|(_, slot)| {
                    slot.state == SlotState::Playing && slot.gain > 0.0 && slot.gain < 1.0
                })
                .map(|(item, _)| *item)
                .collect();
            if overlap.len() == 2 {
                let actual_pair = [self.voices[1].item, self.voices[2].item];
                assert!(
                    actual_pair.iter().all(|item| overlap.contains(item)),
                    "second overlap contains its actual outgoing and incoming sources"
                );
                self.host
                    .with(|host| host.set_tempo(Tempo::new(120.0).expect("Host retarget")))
                    .await
                    .expect("one Host retarget during the actual second overlap");
                data.retarget = Some((now, actual_pair));
                tracing::info!(target: "kithara_test::sync::queue321", phase = "host_retarget_request",
                    frame = frame(now), items = ?actual_pair);
                if let Some(tap) = self.output_wav.as_mut() {
                    tap.mark("Host tempo request 100 to 120 inside actual second overlap");
                }
            }
        }
    }

    fn inspect_bindings(
        &mut self,
        snapshot: &[(LinkedSnapshot<TrackSnapshot>, SlotSnapshot)],
        grid: &BeatGridSnapshot,
        grids: &[(SessionFrame, BeatGridSnapshot)],
        point: (SessionFrame, bool),
        copied: Option<&[(LinkedSnapshot<TrackSnapshot>, SlotSnapshot)]>,
        data: &mut CapturedMusic,
    ) {
        let (now, at_beat) = point;
        let sample_rate = NonZeroU32::new(RATE).expect("source rate");
        for voice in &mut self.voices {
            let Some((linked, slot)) = snapshot.iter().find(|(track, slot)| {
                track.track.item == voice.item
                    && slot.state == SlotState::Playing
                    && slot.mark.zip(slot.source).is_some_and(|(mark, info)| {
                        if copied.is_some() {
                            at_beat && mark.lane.frame > 0 && mark.lane.frame == info.lane_frame
                        } else {
                            mark.lane.frame > info.lane_frame
                        }
                    })
            }) else {
                continue;
            };
            let mark = slot.mark.expect("Playing mark");
            let first = if let Some(copied) = copied {
                assert_eq!(
                    mark.session, now,
                    "the copied boundary is the exact Host beat"
                );
                let seat = linked.track.slot.expect("the real Playing slot");
                let after = copied
                    .iter()
                    .find(|(track, _)| {
                        track.track.item == voice.item && track.track.slot == Some(seat)
                    })
                    .expect("the copied beat retains its actual item and slot");
                let actual = Capture::for_track(u64::from(voice.item), RATE)
                    .expect("the exact native copy history of this item");
                copied_beat(voice.item, seat, *slot, after, &actual)
                    .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
                data.boundary_copy.get_or_insert_with(|| BoundaryCopy {
                    item: voice.item,
                    seat,
                    before: *slot,
                    after: after.clone(),
                });
                *voice
                    .first
                    .get(&mark.lane.segment)
                    .expect("a copied boundary retains the original consumed segment anchor")
            } else {
                *voice.first.entry(mark.lane.segment).or_insert(*slot)
            };
            let inspect = self.deck.observe(|o| {
                let capture = o.capture.as_ref().expect("typed capture");
                let overlapping = capture.starts.len() >= 2
                    && snapshot
                        .iter()
                        .filter(|(_, other)| {
                            other.state == SlotState::Playing
                                && other.gain > 0.0
                                && other.gain < 1.0
                        })
                        .count()
                        == 2;
                overlapping || at_beat
            });
            if !inspect {
                continue;
            }
            let result = self.deck.observe(|o| {
                super::binding::executed_law(&voice.model, grid, grids, o, linked, first, *slot)
            });
            let (speed, failures, mut evidence) = result.unwrap_or_else(|error| {
                self.deck.observe(|o| {
                    let capture = o.capture.as_ref().expect("typed capture");
                    panic!(
                        "item {:?}: {error}; now={now:?}; first={first:?}; \
                         consumed={slot:?}; linked={linked:?}; faults={:?}; \
                         starts={:?}; segments={:?}; consumed_tail={:?}; \
                         speeds={:?}; cues={:?}; receipts={:?}; inputs={:?}; deck_events={:?}",
                        voice.item,
                        capture.faults,
                        capture.starts,
                        capture.segments,
                        capture.consumed.iter().rev().take(8).collect::<Vec<_>>(),
                        o.speeds,
                        o.cue_curves,
                        o.settled,
                        o.owner_inputs,
                        o.timeline
                            .iter()
                            .filter(|event| event["event"] == "deck event")
                            .collect::<Vec<_>>(),
                    );
                })
            });
            if copied.is_some() {
                evidence["copied_boundary"] = serde_json::json!({
                    "item": u64::from(voice.item), "frame": frame(now),
                    "actual_copied_domain": [frame(now), frame(now) + 1],
                    "published_source": format!("{:?}", slot.source),
                    "exact_source_coordinates": [0, 1],
                });
            }
            let mut valid = failures.is_empty();
            evidence["law_failures"] = serde_json::json!(failures);
            data.failures.extend(
                failures
                    .into_iter()
                    .map(|failure| format!("item {:?} at {now:?}: {failure}", voice.item)),
            );
            if !matches!(linked.sync, kithara::link::SyncStatus::Correcting { .. }) {
                let binding =
                    super::binding::check(&voice.model, sample_rate, grid, *slot, speed.as_(), 4)
                        .unwrap_or_else(|error| panic!("item {:?}: {error}", voice.item));
                let failures = binding.failures();
                valid &= failures.is_empty();
                evidence["binding"] = binding.evidence();
                evidence["binding_failures"] = serde_json::json!(failures);
                data.failures.extend(
                    failures
                        .into_iter()
                        .map(|failure| format!("item {:?} at {now:?}: {failure}", voice.item)),
                );
                if valid {
                    voice.bound = Some((*slot, grid.clone(), speed.as_()));
                }
            }
            evidence["passed"] = serde_json::json!(valid);
            if valid && at_beat {
                data.primary_beats.push((frame(now), voice.item));
            }
            data.checks.push(evidence);
            if valid {
                voice.checks += 1;
            }
        }
    }

    async fn render_block(&mut self, frames: usize, data: &mut CapturedMusic) -> Instant {
        let started = Instant::now();
        let output = self.host.render(frames).await;
        let block = self.master.drain();
        assert_eq!(self.master.drops(), 0, "Master tap retains every sample");
        assert_eq!(block.len(), frames * 2, "actual Master frame count");
        assert_eq!(output.len(), block.len(), "actual Output frame count");
        data.output_delta = output
            .iter()
            .zip(&block)
            .map(|(a, b)| (a - b).abs())
            .fold(data.output_delta, f32::max);
        if let Some(tap) = self.output_wav.as_mut() {
            tap.push(&output);
        }
        if let Some(tap) = self.master_wav.as_mut() {
            tap.push(&block);
        }
        data.pcm.extend(block);
        started
    }

    fn publish(
        &mut self,
        data: &CapturedMusic,
        capture: &LinkCapture,
        interval_count: usize,
        required_beats: usize,
        opens: usize,
    ) {
        let (issued, _) = data.retarget.expect("actual Host retarget");
        let events = usdt_trace::events();
        let consumed_slices = events
            .iter()
            .filter(|event| event.target == "kithara_render_probe" && event.probe == "pcm_consumed")
            .count();
        let geometry_events = events
            .iter()
            .filter(|event| {
                event.target == "kithara_render_probe" && event.probe == "pcm_consumed_geometry"
            })
            .count();
        let evidence = serde_json::json!({
            "scenario": "one actual Queue, five musical sources, four crossfades, second-overlap Host retarget",
            "sources": SOURCES, "items": self.voices.iter().map(|voice| u64::from(voice.item)).collect::<Vec<_>>(),
            "capture_origin_session_frame": 0, "frames": data.pcm.len() / 2, "channels": 2, "sample_rate": RATE,
            "start_receipts": format!("{:?}", capture.starts), "outgoing_fades": format!("{:?}", capture.fades),
            "native_segments": format!("{:?}", capture.segments), "faded": format!("{:?}", capture.faded), "ended": format!("{:?}", capture.ended),
            "host_retarget_request_frame": frame(issued), "source_open_count": opens,
            "typed_source_intervals": interval_count, "all_host_beat_voice_checks": required_beats, "exact_pcm": true,
            "negative_controls": ["omit outgoing", "omit incoming", "one-frame source cue", "supplied grid shift"],
            "grid_law_checks": data.checks, "nonblocking_underruns": 0, "tap_drops": self.master.drops(),
            "consumed_capture": {
                "history_events": events.len(), "consumed_slices": consumed_slices,
                "geometry_events": geometry_events, "per_probe_capacity": kithara_test_utils::test::usdt::MAX_EVENTS,
            },
            "source_ready_basis": events.iter().filter(|event| event.probe == "lane_source_ready")
                .map(|event| event.fields().collect::<BTreeMap<_, _>>()).collect::<Vec<_>>(),
            "cadence": "paced at rendered duration; Host max/Warp quantum 512, max pull 511; stop at actual packet interior then final frame and published Host beat boundaries; not fixed-512 deadline stress"
        });
        for tap in [&mut self.output_wav, &mut self.master_wav]
            .into_iter()
            .flatten()
        {
            tap.evidence("queue321", evidence.clone());
        }
        eprintln!(
            "QUEUE321_ACCEPTANCE {}",
            serde_json::json!({"sources": self.voices.len(), "crossfades": capture.faded.len(),
        "frames": data.pcm.len() / 2, "source_opens": opens, "intervals": interval_count, "pcm": "exact-f32", "negative_controls": 4})
        );
    }
}

fn copied_beat(
    item: TrackId,
    seat: Slot,
    before: SlotSnapshot,
    after: &(LinkedSnapshot<TrackSnapshot>, SlotSnapshot),
    capture: &Capture,
) -> Result<(), String> {
    let mark = before.mark.ok_or("boundary has no real PCM mark")?;
    let info = before.source.ok_or("boundary has no source identity")?;
    let span = info
        .source_span
        .ok_or("boundary has no exact upcoming SourceSpan")?;
    let copied_mark = after.1.mark.ok_or("copied beat has no real PCM mark")?;
    let copied_info = after.1.source.ok_or("copied beat has no source identity")?;
    if before.state != SlotState::Playing
        || after.1.state != SlotState::Playing
        || after.0.track.item != item
        || after.0.track.slot != Some(seat)
        || after
            .0
            .track
            .mark
            .is_none_or(|owner| owner.lane.segment != copied_mark.lane.segment)
        || mark.lane.frame == 0
        || mark.lane.frame != info.lane_frame
        || mark.lane.segment != info.segment
        || copied_mark.lane.segment != mark.lane.segment
        || copied_info.segment != info.segment
        || copied_info.spec != info.spec
        || copied_info.source_revision != info.source_revision
        || copied_info.media_origin != info.media_origin
        || copied_mark.session <= mark.session
        || copied_mark.lane.frame.checked_sub(mark.lane.frame)
            != copied_mark.session.frames_since(mark.session)
    {
        return Err(format!(
            "copied beat lost its real item, slot, segment or advancing PCM mark: item={item:?}, seat={seat:?}, before={before:?}, after={after:?}"
        ));
    }
    let at = frame(mark.session);
    let end = at.checked_add(1).ok_or("boundary session overflows")?;
    let covered = capture.covered(at..end)?;
    let (&start, copied) = covered
        .first_key_value()
        .ok_or("boundary has no actual copied sample")?;
    let offset = u64::try_from(at - start).map_err(|error| error.to_string())?;
    if copied.identity.spec != info.spec
        || copied.identity.source_revision != info.source_revision
        || copied.identity.media_origin != info.media_origin
        || copied.source.render_revision() != info.render_revision
        || copied.source.render_revision() != span.render_revision()
        || copied.source.mapping_revision() != info.mapping_revision
        || copied.source.mapping_revision() != span.mapping_revision()
        || copied.source.sample_rate() != span.sample_rate()
        || span.position_at(0) != Some(mark.position)
    {
        return Err(
            "copied beat lost exact source identity, mapping or render revision".to_owned(),
        );
    }
    for advance in 0..=1 {
        let actual = copied
            .source
            .source_ratio_at(offset + advance)
            .ok_or("copied sample has no exact source coordinate")?;
        let published = span
            .source_ratio_at(advance)
            .ok_or("boundary law does not cover its first sample")?;
        if actual != published {
            return Err(format!(
                "copied boundary at {at}+{advance} maps {actual:?}, published law requires {published:?}"
            ));
        }
    }
    Ok(())
}

fn boundary_negative_controls(
    data: &CapturedMusic,
    failures: &mut Vec<String>,
) -> serde_json::Value {
    let Some(BoundaryCopy {
        item,
        seat,
        before,
        after,
    }) = data.boundary_copy.as_ref()
    else {
        failures.push("no real copied packet boundary exercised the boundary controls".to_owned());
        return serde_json::json!({"error": "no real copied boundary"});
    };
    let capture =
        Capture::for_track(u64::from(*item), RATE).expect("actual copied boundary source history");
    copied_beat(*item, *seat, *before, after, &capture)
        .expect("retained actual boundary remains independently verifiable");
    let info = before.source.expect("actual boundary source");
    let span = info.source_span.expect("actual boundary geometry");
    let at = frame(before.mark.expect("actual boundary mark").session);
    let trace = super::consumed::check_trace_negatives(u64::from(*item), RATE, at..at + 1);
    let mut wrong_item = after.clone();
    wrong_item.0.track.item = *data
        .selected_items
        .iter()
        .find(|candidate| **candidate != *item)
        .expect("another actual Queue item");
    let mut wrong_segment = after.clone();
    let segment = info.segment.next();
    wrong_segment
        .1
        .source
        .as_mut()
        .expect("actual source")
        .segment = segment;
    wrong_segment
        .1
        .mark
        .as_mut()
        .expect("actual mark")
        .lane
        .segment = segment;
    wrong_segment.0.track.mark = wrong_segment.1.mark;
    let mut wrong_owner_segment = after.clone();
    wrong_owner_segment
        .0
        .track
        .mark
        .as_mut()
        .expect("actual owner mark")
        .lane
        .segment = segment;
    let mut missing_owner_mark = after.clone();
    missing_owner_mark.0.track.mark = None;
    let mut wrong_source = *before;
    wrong_source
        .source
        .as_mut()
        .expect("actual source")
        .source_revision = info
        .source_revision
        .checked_add(1)
        .expect("different source revision");
    let mut wrong_render = *before;
    let revised = info
        .render_revision
        .checked_add(1)
        .expect("different render revision");
    let revised_info = wrong_render.source.as_mut().expect("actual source");
    revised_info.render_revision = revised;
    revised_info.source_span = Some(span.with_render_revision(revised));
    let mut wrong_slope = *before;
    let origin = span.source_ratio_at(0).expect("actual boundary coordinate");
    let altered = span
        .scaled_from(origin, origin, (2, std::num::NonZeroU128::MIN))
        .expect("different slope through the same exact origin");
    assert_eq!(altered.source_ratio_at(0), span.source_ratio_at(0));
    assert_ne!(altered.source_ratio_at(1), span.source_ratio_at(1));
    wrong_slope
        .source
        .as_mut()
        .expect("actual source")
        .source_span = Some(altered);
    let outcomes = [
        ("missing_duplicate_or_orphan_geometry", trace.is_ok()),
        (
            "wrong_item",
            copied_beat(*item, *seat, *before, &wrong_item, &capture).is_err(),
        ),
        (
            "reused_segment",
            copied_beat(*item, *seat, *before, &wrong_segment, &capture).is_err(),
        ),
        (
            "wrong_owner_segment",
            copied_beat(*item, *seat, *before, &wrong_owner_segment, &capture).is_err(),
        ),
        (
            "missing_owner_segment_mark",
            copied_beat(*item, *seat, *before, &missing_owner_mark, &capture).is_err(),
        ),
        (
            "wrong_source_revision",
            copied_beat(*item, *seat, wrong_source, after, &capture).is_err(),
        ),
        (
            "new_render_revision_at_same_origin",
            copied_beat(*item, *seat, wrong_render, after, &capture).is_err(),
        ),
        (
            "different_slope_at_same_origin_and_revision",
            copied_beat(*item, *seat, wrong_slope, after, &capture).is_err(),
        ),
    ];
    for (name, rejected) in outcomes {
        if !rejected {
            failures.push(format!("copied-boundary negative accepted {name}"));
        }
    }
    serde_json::json!({
        "item": u64::from(*item), "frame": at,
        "trace_error": trace.err(),
        "outcomes": outcomes.into_iter().collect::<BTreeMap<_, _>>()
    })
}

fn pull_frames(
    consumed: &[(TrackId, SlotSnapshot)],
    grid: &BeatGridSnapshot,
    now: SessionFrame,
) -> (usize, i64) {
    let mut beat = host_beat(grid, frame(now)).floor() + 1.0;
    let mut boundary = beat_frame(grid, beat);
    while boundary <= frame(now) {
        beat += 1.0;
        boundary = beat_frame(grid, beat);
    }
    let frames = consumed
        .iter()
        .filter(|(_, slot)| slot.state == SlotState::Playing)
        .filter_map(|(_, slot)| Some((slot.mark?, slot.source?)))
        .filter_map(|(mark, info)| {
            info.source_span.and_then(|span| {
                info.lane_frame
                    .checked_add(span.output_frames())?
                    .checked_sub(mark.lane.frame)
            })
        })
        .map(|left| {
            usize::try_from(if left > 1 { left - 1 } else { 1 })
                .expect("observed packet interior or final frame")
        })
        .fold(BLOCK - 1, usize::min)
        .min(usize::try_from(boundary - frame(now)).expect("next actual Host beat boundary"));
    (frames, boundary)
}

fn assert_transport(
    o: &LinkObservation,
    voices: &[Voice],
    data: &CapturedMusic,
    failures: &mut Vec<String>,
) {
    let crossfade = CrossfadeSettings::default();
    let capture = o.capture.as_ref().expect("capture");
    assert_eq!(capture.starts.len(), 5, "all five sources actually start");
    assert_eq!(o.loads.len(), 5, "one actual Load per source");
    assert_eq!(
        data.answered.len(),
        5,
        "all five own their delivered grid answer"
    );
    assert_eq!(
        data.selected_items,
        voices.iter().map(|voice| voice.item).collect::<Vec<_>>(),
        "public Queue selection publishes all five items in playback order"
    );
    assert!(
        o.seeks.is_empty(),
        "automatic transitions never reopen a source to seek"
    );
    assert!(
        o.placements.iter().all(|placement| {
            voices.iter().any(|voice| {
                voice.item == placement.item && placement.to == first_strong_beat(voice)
            })
        }) && o.cue_curves.iter().all(|cue| {
            voices
                .iter()
                .any(|voice| voice.item == cue.item && cue.position == first_strong_beat(voice))
        }),
        "every initial or reprepared cue uses that source's supplied first strong beat"
    );
    for voice in voices {
        let loaded = o
            .source_clocks
            .iter()
            .find(|(owner, _)| *owner == voice.item)
            .expect("actual Load publishes its initial source clock")
            .1;
        assert_eq!(
            loaded.frame_offset, 0,
            "named fixture Load actually opens at source zero"
        );
    }
    assert!(
        capture.faults.is_empty(),
        "nonblocking source playback has no underrun/fault: {:?}",
        capture.faults
    );
    assert_eq!(capture.faded.len(), 4, "four real outgoing crossfades");
    assert_eq!(capture.ended.len(), 1, "last source reaches natural end");
    for (index, voice) in voices.iter().enumerate() {
        assert_eq!(
            capture.starts[index].0, voice.item,
            "source order through the same Queue"
        );
        if voice.checks == 0 || voice.bound.is_none() {
            failures.push(format!(
                "item {:?}: every source must have a valid physical grid binding",
                voice.item
            ));
        }
        assert_eq!(
            o.loads
                .iter()
                .filter(|(item, _)| *item == voice.item)
                .count(),
            1
        );
        if index > 0 {
            let (previous, _, at, settings, _) = capture
                .fades
                .iter()
                .find(|(item, _, _, _, dir)| {
                    *item == voices[index - 1].item && *dir == FadeDir::Out
                })
                .expect("native outgoing Fade");
            assert_eq!(*previous, voices[index - 1].item);
            assert_eq!(
                *at, capture.starts[index].2,
                "both crossfade voices share the actual Applied F"
            );
            assert_eq!(*settings, crossfade, "native outgoing envelope settings");
            assert_eq!(
                capture.starts[index].3,
                Fade::Crossfade(*settings),
                "incoming declared envelope"
            );
            let end = capture
                .faded
                .iter()
                .find(|(owner, ..)| owner == previous)
                .expect("native Faded")
                .2;
            assert_eq!(
                frame(end) - frame(*at),
                i64::from(RATE),
                "one-second envelope survives Host retarget"
            );
        }
    }
}

fn assert_reuse(
    o: &LinkObservation,
    voices: &[Voice],
    segments: &[(TrackId, Slot, SessionFrame, u64, u64)],
) {
    let capture = o.capture.as_ref().expect("capture");
    assert!(
        segments.iter().all(|(item, _, applied, _, _)| capture
            .starts
            .iter()
            .any(|(owner, _, start, _)| item == owner && applied <= start)),
        "UNSUPPORTED_ORACLE_WITNESS: named fixture jumps to a new source segment after its actual Start"
    );
    assert_eq!(
        capture.starts[0].1, capture.starts[4].1,
        "fifth source reuses first slot"
    );
    let first_custody = segments
        .iter()
        .find(|(item, ..)| *item == voices[0].item)
        .expect("first custody");
    let reused_custody = segments
        .iter()
        .find(|(item, ..)| *item == voices[4].item)
        .expect("fifth custody");
    assert_ne!(
        first_custody.0, reused_custody.0,
        "reused slot belongs to the fifth item"
    );
    let first_load = o
        .loads
        .iter()
        .find(|(item, _)| *item == voices[0].item)
        .expect("first actual Load")
        .1;
    let reused_load = o
        .loads
        .iter()
        .find(|(item, _)| *item == voices[4].item)
        .expect("fifth actual Load")
        .1;
    assert_ne!(
        first_load, reused_load,
        "slot reuse owns a distinct actual Load"
    );
    assert_ne!(
        voices[0].model.as_raw(),
        voices[4].model.as_raw(),
        "slot reuse owns the fifth source grid"
    );
}

fn assert_gains(o: &LinkObservation, mixer: DeckMixerConfig) {
    let capture = o.capture.as_ref().expect("capture");
    for &(at, item, _, slot) in &capture.consumed {
        if slot.state != SlotState::Playing {
            continue;
        }
        let Some(mark) = slot.mark else {
            continue;
        };
        let Some((_, _, start, _)) = capture.starts.iter().find(|(owner, ..)| *owner == item)
        else {
            continue;
        };
        if mark.lane.frame == 0 || mark.session <= *start {
            continue;
        }
        let expected = envelope(capture, item, frame(mark.session) - 1, mixer);
        assert_eq!(
            slot.gain, expected,
            "item {item:?} published gain at {at:?} follows the native envelope"
        );
    }
}

fn assert_retarget(o: &LinkObservation, data: &CapturedMusic) {
    let capture = o.capture.as_ref().expect("capture");
    let (issued, overlap_items) = data
        .retarget
        .expect("a real Host retarget inside the second actual overlap");
    let end = capture
        .faded
        .iter()
        .find(|(owner, ..)| *owner == overlap_items[0])
        .expect("actual second outgoing Faded")
        .2;
    assert!(
        issued < end,
        "request itself occurs before the outgoing native Faded"
    );
    let retimes: Vec<_> = o
        .owner_inputs
        .iter()
        .enumerate()
        .filter(|(_, input)| {
            matches!(
                input.kind,
                kithara_integration_tests::offline::linked::LinkOwnerInputKind::Retime
            ) && input.at.is_some_and(|at| at >= issued)
        })
        .collect();
    assert_eq!(retimes.len(), 1, "one actual owned Host retarget input");
    let retime_input = retimes[0].0;
    for item in overlap_items {
        let applied: Vec<_> = o.speeds.iter().filter(|speed| speed.item == item
                && speed.host_input == Some(retime_input))
                .filter_map(|speed| o.placement_receipts.iter().find_map(|(owner, origin, receipt)|
                    (*owner == item && *origin == kithara_integration_tests::offline::linked::LinkReceiptOrigin::Lane)
                    .then_some(receipt).and_then(|receipt| match receipt {
                        kithara::play::Settled::Applied { seq, at } if *seq == speed.seq && *at >= issued => Some(*at),
                        _ => None,
                    }))).collect();
        assert!(
            applied.iter().any(|at| *at < end),
            "both overlap voices actually apply the retarget before Faded"
        );
    }
}

fn native_segments(
    o: &LinkObservation,
    voices: &[Voice],
) -> Vec<(TrackId, Slot, SessionFrame, u64, u64)> {
    let attached: Vec<_> = usdt_trace::events()
        .into_iter()
        .filter(|event| event.probe == "deck_attached" && event.target == "kithara_render_probe")
        .map(|event| {
            let item = TrackId::from(event.field("track_id").expect("native item"));
            let slot = Slot::new(
                u16::try_from(event.field("slot").expect("native slot")).expect("slot fits"),
            );
            let at = SessionFrame::new(
                i64::try_from(event.field("at").expect("native frame")).expect("nonnegative frame"),
            );
            let segment = event.field("segment").expect("native segment");
            let seq = event.field("seq").expect("native batch sequence");
            assert!(
                o.deck.iter().any(
                    |(actual, verdict)| actual.get() == seq && *verdict == LinkVerdict::Applied
                ),
                "an actual attachment also receives its Applied answer"
            );
            (item, slot, at, segment, seq)
        })
        .collect();
    assert_eq!(attached.len(), voices.len(), "one actual Attach per source");
    for voice in voices {
        assert_eq!(
            attached
                .iter()
                .filter(|(item, ..)| *item == voice.item)
                .count(),
            1,
            "every source owns exactly one native attachment"
        );
    }
    for (index, &(_, slot, at, _, _)) in attached.iter().enumerate() {
        assert!(
            attached[..index]
                .iter()
                .all(|(_, seat, applied, _, _)| *seat != slot || *applied != at),
            "UNSUPPORTED_ORACLE_WITNESS: same-frame slot reuse needs an execution ordinal"
        );
    }
    let capture = o.capture.as_ref().expect("capture");
    for &(item, slot, at, _, _) in &capture.segments {
        assert!(
            !attached
                .iter()
                .any(|(_, seat, applied, _, _)| *seat == slot && *applied == at),
            "UNSUPPORTED_ORACLE_WITNESS: same-frame Attach/Adopt needs an execution ordinal"
        );
        let native = attached
            .iter()
            .filter(|(_, seat, applied, _, _)| *seat == slot && *applied < at)
            .max_by_key(|(_, _, applied, _, _)| *applied)
            .expect("every Adopt follows a real attachment in its slot");
        assert_eq!(
            item, native.0,
            "an Adopt receipt retains the actual RT slot owner"
        );
    }
    let mut segments = attached;
    segments.extend_from_slice(&capture.segments);
    segments.sort_by_key(|(_, _, at, _, _)| *at);
    assert!(segments.len() <= 16, "bounded native segment custody");
    segments
}

fn native_capture(
    case: &QueueMusic,
    data: &CapturedMusic,
    failures: &mut Vec<String>,
) -> LinkCapture {
    case.deck.observe(|o| {
        let segments = native_segments(o, &case.voices);
        assert_transport(o, &case.voices, data, failures);
        assert_reuse(o, &case.voices, &segments);
        assert_gains(o, case.mixer);
        assert_retarget(o, data);
        let capture = o.capture.as_ref().expect("capture");
        LinkCapture {
            consumed: capture.consumed.clone(),
            starts: capture.starts.clone(),
            fades: capture.fades.clone(),
            segments,
            faded: capture.faded.clone(),
            ended: capture.ended.clone(),
            faults: capture.faults.clone(),
        }
    })
}

fn assert_all_beats(
    voices: &[Voice],
    capture: &LinkCapture,
    final_grid: &BeatGridSnapshot,
    primary_beats: &[(i64, TrackId)],
    domain: &Range<i64>,
    failures: &mut Vec<String>,
) -> usize {
    let first_beat: i64 = host_beat(final_grid, domain.start).ceil().as_();
    let last_beat: i64 = host_beat(final_grid, domain.end).floor().as_();
    let mut required_beats = 0;
    for beat in first_beat..=last_beat {
        let at = beat_frame(final_grid, beat.as_());
        for voice in voices {
            let start = capture
                .starts
                .iter()
                .find(|(item, ..)| *item == voice.item)
                .expect("native Start")
                .2;
            let end = native_end(capture, voice.item);
            if at > frame(start) && at < end {
                required_beats += 1;
                if !primary_beats.contains(&(at, voice.item)) {
                    failures.push(format!(
                        "item {:?}: Host beat {beat} at actual session frame {at} has no real consumed grid/law check",
                        voice.item
                    ));
                }
            }
        }
    }
    if required_beats == 0 {
        failures.push("all-beat census is nonempty".to_owned());
    }
    required_beats
}

fn assert_negative_controls(
    voices: &[Voice],
    capture: &LinkCapture,
    intervals: &[Interval],
    mixer: DeckMixerConfig,
    pcm: &[f32],
    failures: &mut Vec<String>,
) -> serde_json::Value {
    let mut outcomes = serde_json::Map::new();
    let sample_rate = NonZeroU32::new(RATE).expect("source rate");
    let second_start: NativeStart = capture.starts[2];
    let overlap_end = capture
        .faded
        .iter()
        .find(|(item, ..)| *item == voices[1].item)
        .expect("second outgoing Faded")
        .2;
    let overlap = (frame(second_start.2) + 1)..(frame(overlap_end) - 1);
    let actual = &pcm[usize::try_from(overlap.start).expect("overlap start") * 2
        ..usize::try_from(overlap.end).expect("overlap end") * 2];
    for (omit, name) in [(1, "omit_outgoing"), (2, "omit_incoming")] {
        let negative = reference(
            voices,
            capture,
            intervals,
            mixer,
            overlap.clone(),
            Some(omit),
            None,
        );
        let rejected = super::control::compare_pcm(actual, &negative).is_err();
        outcomes.insert(name.to_owned(), serde_json::json!(rejected));
        if !rejected {
            failures.push(format!(
                "omitting either actual overlapping source must fail: {name}"
            ));
        }
    }
    let shifted = reference(
        voices,
        capture,
        intervals,
        mixer,
        overlap.clone(),
        None,
        Some(2),
    );
    let rejected = super::control::compare_pcm(actual, &shifted).is_err();
    outcomes.insert(
        "one_frame_source_cue".to_owned(),
        serde_json::json!(rejected),
    );
    if !rejected {
        failures.push(
            "a one-frame wrong source cue must fail with retained metadata unchanged".to_owned(),
        );
    }
    let Some((slot, grid, speed)) = voices[2].bound.as_ref() else {
        failures
            .push("supplied-grid negative has no valid actual settled source binding".to_owned());
        outcomes.insert("supplied_grid_shift".to_owned(), serde_json::Value::Null);
        return serde_json::Value::Object(outcomes);
    };
    let mut displaced = voices[2].model.as_raw().clone();
    for beat in &mut displaced.beats {
        beat.at += 1.0 / f64::from(RATE);
    }
    for downbeat in &mut displaced.downbeats {
        downbeat.at += 1.0 / f64::from(RATE);
    }
    let displaced = BeatGridModel::try_from(displaced).expect("valid wrongly placed supplied grid");
    let negative = super::binding::check(&displaced, sample_rate, grid, *slot, *speed, 4);
    let rejected =
        negative.is_err() || negative.is_ok_and(|binding| !binding.failures().is_empty());
    outcomes.insert(
        "supplied_grid_shift".to_owned(),
        serde_json::json!(rejected),
    );
    if !rejected {
        failures.push("wrong supplied grid must fail independently of waveform peaks".to_owned());
    }
    serde_json::Value::Object(outcomes)
}

fn assert_music(
    voices: &[Voice],
    capture: &LinkCapture,
    pcm: &[f32],
    final_grid: &BeatGridSnapshot,
    primary_beats: &[(i64, TrackId)],
    mixer: DeckMixerConfig,
    failures: &mut Vec<String>,
) -> (Vec<Interval>, usize, serde_json::Value) {
    let wall = Instant::now();
    let intervals = intervals(voices, capture);
    let domain = 0..i64::try_from(pcm.len() / 2).expect("captured frames");
    let before_beats = failures.len();
    let required_beats = assert_all_beats(
        voices,
        capture,
        final_grid,
        primary_beats,
        &domain,
        failures,
    );
    let beat_failures = failures.len() - before_beats;
    tracing::info!(target: "kithara_test::sync::queue321", phase = "full_reference_begin");
    let expected = reference(
        voices,
        capture,
        &intervals,
        mixer,
        domain.clone(),
        None,
        None,
    );
    tracing::info!(target: "kithara_test::sync::queue321", phase = "full_reference_complete",
        wall_s = wall.elapsed().as_secs_f64(), samples = expected.len());
    let comparison = super::control::compare_pcm(pcm, &expected);
    let exact_pcm = comparison.is_ok();
    if let Err(error) = comparison {
        failures.push(format!(
            "exact complete source/Host mixed Master PCM: {error}"
        ));
    }
    (
        intervals,
        required_beats,
        serde_json::json!({
            "all_beats": {"required": required_beats, "failures": beat_failures, "passed": beat_failures == 0},
            "exact_master_pcm": exact_pcm,
        }),
    )
}

#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(300)))]
#[kithara_test_utils::kithara::hang_watchdog(timeout = Duration::from_secs(5), ctx = String)]
async fn one_queue_crossfades_five_music_tracks_and_retargets_host_during_overlap() {
    let _trace = usdt_trace::scope();
    let wall = Instant::now();
    tracing::info!(target: "kithara_test::sync::queue321", phase = "setup_begin");
    let mut case = QueueMusic::new().await;
    case.append_sources().await;
    tracing::info!(target: "kithara_test::sync::queue321", phase = "render_begin",
        wall_s = wall.elapsed().as_secs_f64(), frame = case.host.position());
    let data = case.run().await;
    hang_tick!(
        "queue321: validating native starts, ends, exact SourceSpan and physical mixed PCM"
            .to_owned()
    );
    tracing::info!(target: "kithara_test::sync::queue321", phase = "native_oracle_begin",
        wall_s = wall.elapsed().as_secs_f64(), frame = case.host.position());
    let mut failures = data.failures.clone();
    let capture = native_capture(&case, &data, &mut failures);
    tracing::info!(target: "kithara_test::sync::queue321", phase = "native_oracle_complete",
        wall_s = wall.elapsed().as_secs_f64());
    let opens = usdt_trace::events()
        .iter()
        .filter(|event| event.probe == "source_opened")
        .count();
    assert_eq!(
        opens, 5,
        "five Loads correspond to exactly five real source opens"
    );
    assert!(
        data.output_delta > 0.0,
        "Output WAV carries actual Host metronome separately from Master"
    );
    let final_grid = case.host.session_grid().await;
    let voices = std::mem::take(&mut case.voices);
    let mixer = case.mixer;
    hang_reset!();
    let (voices, data, capture, interval_count, required_beats, final_checks, failures) =
        kithara::platform::tokio::task::spawn_blocking(move || {
            let (intervals, required_beats, music_checks) = assert_music(
                &voices,
                &capture,
                &data.pcm,
                &final_grid,
                &data.primary_beats,
                mixer,
                &mut failures,
            );
            tracing::info!(target: "kithara_test::sync::queue321", phase = "negative_controls_begin",
                wall_s = wall.elapsed().as_secs_f64());
            let negative_controls = assert_negative_controls(
                &voices,
                &capture,
                &intervals,
                mixer,
                &data.pcm,
                &mut failures,
            );
            let boundary_controls = boundary_negative_controls(&data, &mut failures);
            tracing::info!(target: "kithara_test::sync::queue321", phase = "negative_controls_complete",
                wall_s = wall.elapsed().as_secs_f64());
            let final_checks = serde_json::json!({
                "frames": data.pcm.len() / 2,
                "intervals": intervals.len(),
                "required_host_beat_voice_checks": required_beats,
                "online_failure_count": data.failures.len(),
                "valid_binding_checks": voices.iter()
                    .map(|voice| serde_json::json!({"item": u64::from(voice.item), "checks": voice.checks, "settled_binding": voice.bound.is_some()}))
                    .collect::<Vec<_>>(),
                "primary_beats": data.primary_beats.iter()
                    .map(|(at, item)| serde_json::json!({"frame": at, "item": u64::from(*item)}))
                    .collect::<Vec<_>>(),
                "grid_law_checks": data.checks,
                "music": music_checks,
                "negative_control_count": 4,
                "negative_controls": negative_controls,
                "boundary_controls": boundary_controls,
                "failures": &failures,
                "passed": failures.is_empty(),
            });
            (
                voices,
                data,
                capture,
                intervals.len(),
                required_beats,
                final_checks,
                failures,
            )
        })
        .await
        .expect("complete Queue PCM reference analysis must join");
    case.voices = voices;
    for tap in [&mut case.output_wav, &mut case.master_wav]
        .into_iter()
        .flatten()
    {
        tap.evidence("queue321_final_checks", final_checks.clone());
    }
    assert!(failures.is_empty(), "queue321 final checks: {failures:#?}");
    case.publish(&data, &capture, interval_count, required_beats, opens);
    hang_reset!();
}
