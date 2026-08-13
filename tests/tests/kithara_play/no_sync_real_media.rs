#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

use kithara::{
    audio::{StretchControls, StretchKind},
    bufpool::{BytePool, PcmPool},
    events::{
        AudioEvent, BusEvent, DecoderEvent, DownloaderEvent, Event, EventBus, EventReceiver,
        FileEvent, HlsEvent, ItemEvent, PlaybackResamplerKind, PlayerEvent,
    },
    hls::AbrMode,
    platform::{
        sync::Arc,
        time::{self, Duration},
        tokio::sync::broadcast::error::TryRecvError,
    },
    play::{
        Cmd, PlayerConfig, PlayerImpl, Reply, Resource, ResourceConfig, SelectTransition,
        SessionDispatcher, SessionError, apply_mix,
    },
};
use kithara_integration_tests::{
    TestServerHelper,
    audio_artifact::write_audio_artifact,
    cochlea::{CochleaReport, assert_oracle_load_bearing},
    memory_asset_store,
    offline::OfflineSession,
};
use serde::Serialize;

const CHANNELS: u16 = 2;
const SOURCE_RATE: u32 = 44_100;
const BLOCK_FRAMES: usize = 512;
const CAPTURE_SECS: u32 = 2;
const MAX_WARMUP_SECS: u32 = 8;
const MIN_WARMUP_POSITION_SECS: f64 = 0.5;
const MIX_HEADROOM: f32 = 0.2;
const ACTIVE_RMS: f64 = 1.0e-5;
const POSITION_TOLERANCE_SECS: f64 = 0.15;
const EXACT_ZERO_RUN_LIMIT_FRAMES: usize = 8;
const MIN_BOUNDARY_JUMP: f32 = 0.05;
const BOUNDARY_OUTLIER_RATIO: f32 = 6.0;
const PRELOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
enum Media {
    Mp3(&'static str),
    Hls,
}

impl Media {
    const fn label(self) -> &'static str {
        match self {
            Self::Mp3(name) => name,
            Self::Hls => "hls/master.m3u8",
        }
    }
}

struct Case {
    label: &'static str,
    host_rate: u32,
    media: &'static [Media],
}

const MP3_ONE: &[Media] = &[Media::Mp3("test.mp3")];
const MP3_TWO: &[Media] = &[Media::Mp3("test.mp3"), Media::Mp3("track.mp3")];
const MP3_FOUR: &[Media] = &[
    Media::Mp3("test.mp3"),
    Media::Mp3("track.mp3"),
    Media::Mp3("test.mp3"),
    Media::Mp3("track.mp3"),
];
const HLS_ONE: &[Media] = &[Media::Hls];
const HLS_MP3_TWO: &[Media] = &[Media::Hls, Media::Mp3("track.mp3")];
const HLS_MP3_FOUR: &[Media] = &[
    Media::Hls,
    Media::Mp3("track.mp3"),
    Media::Hls,
    Media::Mp3("test.mp3"),
];

const CASES: &[Case] = &[
    Case {
        label: "no-sync-mp3-one-44100",
        host_rate: 44_100,
        media: MP3_ONE,
    },
    Case {
        label: "no-sync-mp3-distinct-two-48000",
        host_rate: 48_000,
        media: MP3_TWO,
    },
    Case {
        label: "no-sync-mp3-alternating-four-44100",
        host_rate: 44_100,
        media: MP3_FOUR,
    },
    Case {
        label: "no-sync-hls-one-48000",
        host_rate: 48_000,
        media: HLS_ONE,
    },
    Case {
        label: "no-sync-hls-mp3-distinct-two-44100",
        host_rate: 44_100,
        media: HLS_MP3_TWO,
    },
    Case {
        label: "no-sync-hls-mp3-alternating-four-48000",
        host_rate: 48_000,
        media: HLS_MP3_FOUR,
    },
];

struct Deck {
    player: Arc<PlayerImpl>,
    controls: Arc<StretchControls>,
    events: EventReceiver,
    observation: DeckObservation,
}

#[derive(Default, Serialize)]
struct DeckObservation {
    decoder_changes: usize,
    decoder_sample_rates: Vec<u32>,
    decoder_channels: Vec<u16>,
    decoder_variants: Vec<Option<u32>>,
    playback_resamplers: Vec<ResamplerObservation>,
    final_position_secs: f64,
    hls: bool,
    label: &'static str,
}

#[derive(Serialize)]
struct ResamplerObservation {
    active: bool,
    backend: &'static str,
    host_sample_rate: u32,
    source_sample_rate: u32,
}

#[derive(Serialize)]
struct ArtifactManifest<'a> {
    case: &'a str,
    media: Vec<&'static str>,
    deck_count: usize,
    host_sample_rate: u32,
    channels: u16,
    requested_frames: usize,
    captured_frames: usize,
    mix_tap_drops: u64,
    mix_tap_matches_output: bool,
    sample_continuity: Option<&'a SampleContinuityReport>,
    cochlea: Option<&'a CochleaReport>,
    decks: &'a [DeckObservation],
    failures: &'a [String],
}

#[derive(Serialize)]
struct SampleContinuityReport {
    discontinuity_boundaries: Vec<usize>,
    longest_exact_zero_run_frames: usize,
    max_boundary_jump: f32,
    p99_adjacent_jump: f32,
    repeated_block_boundaries: Vec<usize>,
}

struct OracleReports {
    cochlea: Option<CochleaReport>,
    sample_continuity: Option<SampleContinuityReport>,
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(300))
)]
async fn no_sync_real_media_matrix_is_continuous_and_unsynchronized() {
    let server = TestServerHelper::new().await;
    let mut failures = Vec::new();
    for case in CASES {
        failures.extend(
            run_case(case, &server)
                .await
                .into_iter()
                .map(|failure| format!("{}: {failure}", case.label)),
        );
    }
    assert!(
        failures.is_empty(),
        "no-SYNC real-media matrix failed:\n{}",
        failures.join("\n"),
    );
}

async fn run_case(case: &Case, server: &TestServerHelper) -> Vec<String> {
    let session = Arc::new(OfflineSession::new_manual());
    let mut failures = Vec::new();

    let mut decks = Vec::with_capacity(case.media.len());
    for (deck_index, media) in case.media.iter().copied().enumerate() {
        decks.push(prepare_deck(case, deck_index, media, server, &session).await);
    }

    start_decks(case, &decks, &mut failures);
    record_transport_state(&session, "before first render", &mut failures);
    drain_all_events(&mut decks, "startup", &mut failures);
    warm_to_steady_state(case, &session, &mut decks, &mut failures).await;

    let capture_blocks = blocks_for_secs(case.host_rate, CAPTURE_SECS);
    let requested_frames = capture_blocks * BLOCK_FRAMES;
    let mut tap = session
        .enable_mix_tap(requested_frames * usize::from(CHANNELS) + BLOCK_FRAMES)
        .unwrap_or_else(|error| panic!("{}: enable final-mix tap: {error}", case.label));
    let positions_before: Vec<f64> = decks
        .iter()
        .map(|deck| deck.player.position_seconds().unwrap_or(0.0))
        .collect();

    let mut capture = Vec::with_capacity(requested_frames * usize::from(CHANNELS));
    let mut silent_blocks = Vec::new();
    for block_index in 0..capture_blocks {
        let block = render_paced(&session, &decks, case.host_rate).await;
        inspect_block(case, block_index, &block, &mut silent_blocks, &mut failures);
        capture.extend_from_slice(&block);
        drain_all_events(&mut decks, "capture", &mut failures);
    }
    for deck in &decks {
        deck.player.process_notifications();
    }
    drain_all_events(&mut decks, "capture final drain", &mut failures);

    let tapped = tap.drain();
    let tap_drops = tap.drops();
    let captured_frames = capture.len() / usize::from(CHANNELS);
    let tap_matches = assess_capture(
        case,
        &capture,
        &tapped,
        tap_drops,
        requested_frames,
        &silent_blocks,
        &mut failures,
    );
    assess_decks(
        case,
        &mut decks,
        &positions_before,
        requested_frames,
        &mut failures,
    );
    record_transport_state(&session, "after capture", &mut failures);
    let oracles = assess_audio(case, &capture, &mut failures);

    let observations: Vec<DeckObservation> =
        decks.into_iter().map(|deck| deck.observation).collect();
    let manifest = ArtifactManifest {
        case: case.label,
        media: case.media.iter().map(|media| media.label()).collect(),
        deck_count: case.media.len(),
        host_sample_rate: case.host_rate,
        channels: CHANNELS,
        requested_frames,
        captured_frames,
        mix_tap_drops: tap_drops,
        mix_tap_matches_output: tap_matches,
        sample_continuity: oracles.sample_continuity.as_ref(),
        cochlea: oracles.cochlea.as_ref(),
        decks: &observations,
        failures: &failures,
    };
    write_audio_artifact(
        case.label,
        case.host_rate,
        CHANNELS,
        &[("final-mix", &capture)],
        &manifest,
    )
    .unwrap_or_else(|error| panic!("{}: write optional audio artifact: {error}", case.label));
    failures
}

fn assess_capture(
    case: &Case,
    capture: &[f32],
    tapped: &[f32],
    tap_drops: u64,
    requested_frames: usize,
    silent_blocks: &[usize],
    failures: &mut Vec<String>,
) -> bool {
    let expected_samples = requested_frames * usize::from(CHANNELS);
    if capture.len() != expected_samples {
        failures.push(format!(
            "{}: final PCM shape was {} samples, expected {expected_samples}",
            case.label,
            capture.len(),
        ));
    }
    if !silent_blocks.is_empty() {
        failures.push(format!(
            "{}: capture contained silent/near-silent callback blocks at {silent_blocks:?}",
            case.label,
        ));
    }
    if tap_drops != 0 {
        failures.push(format!(
            "{}: final-mix tap dropped {tap_drops} samples",
            case.label,
        ));
    }
    let tap_matches = tapped == capture;
    if !tap_matches {
        failures.push(format!(
            "{}: final-mix tap did not match graph output bit-exactly (tap={}, output={})",
            case.label,
            tapped.len(),
            capture.len(),
        ));
    }
    tap_matches
}

fn assess_decks(
    case: &Case,
    decks: &mut [Deck],
    positions_before: &[f64],
    requested_frames: usize,
    failures: &mut Vec<String>,
) {
    let duration = f64::from(u32::try_from(requested_frames).expect("capture frames fit u32"))
        / f64::from(case.host_rate);
    for (deck_index, (deck, before)) in decks
        .iter_mut()
        .zip(positions_before.iter().copied())
        .enumerate()
    {
        let after = deck.player.position_seconds().unwrap_or(0.0);
        deck.observation.final_position_secs = after;
        let advance = after - before;
        if (advance - duration).abs() > POSITION_TOLERANCE_SECS {
            failures.push(format!(
                "{} deck {deck_index} ({}): media position advanced {advance:.6}s over {duration:.6}s of output",
                case.label, deck.observation.label,
            ));
        }
        validate_deck(case, deck_index, deck, failures);
    }
}

fn assess_audio(case: &Case, capture: &[f32], failures: &mut Vec<String>) -> OracleReports {
    let finite = capture.iter().all(|sample| sample.is_finite());
    let sample_continuity = finite.then(|| measure_sample_continuity(capture));
    if let Some(report) = &sample_continuity {
        if report.longest_exact_zero_run_frames >= EXACT_ZERO_RUN_LIMIT_FRAMES {
            failures.push(format!(
                "{}: final mix contained an exact-zero run of {} frames",
                case.label, report.longest_exact_zero_run_frames,
            ));
        }
        if !report.repeated_block_boundaries.is_empty() {
            failures.push(format!(
                "{}: final mix repeated callback blocks at frame boundaries {:?}",
                case.label, report.repeated_block_boundaries,
            ));
        }
        if !report.discontinuity_boundaries.is_empty() {
            failures.push(format!(
                "{}: final mix had callback-boundary jump outliers at frames {:?} (max={:.6}, adjacent_p99={:.6})",
                case.label,
                report.discontinuity_boundaries,
                report.max_boundary_jump,
                report.p99_adjacent_jump,
            ));
        }
    }

    let cochlea = finite.then(|| CochleaReport::measure(capture, CHANNELS, case.host_rate));
    if let Some(report) = &cochlea {
        if report.silent_segments > 0 {
            failures.push(format!(
                "{}: Cochlea found {} silent segments in the steady final mix",
                case.label, report.silent_segments,
            ));
        }
        if report.clipped_samples > 0 || report.true_peak_over_0dbtp {
            failures.push(format!(
                "{}: conservative mix clipped (samples={}, true_peak_over_0dbtp={})",
                case.label, report.clipped_samples, report.true_peak_over_0dbtp,
            ));
        }
    } else {
        failures.push(format!(
            "{}: final PCM contained non-finite samples, so Cochlea could not analyse it",
            case.label,
        ));
    }

    if let Some(report) = &cochlea
        && report.clipped_samples == 0
        && capture.len() >= 2 * BLOCK_FRAMES * usize::from(CHANNELS)
        && catch_unwind(AssertUnwindSafe(|| {
            assert_oracle_load_bearing(capture, CHANNELS, case.host_rate, BLOCK_FRAMES);
        }))
        .is_err()
    {
        failures.push(format!(
            "{}: Cochlea comparator accepted its injected dropout or click",
            case.label,
        ));
    }

    OracleReports {
        cochlea,
        sample_continuity,
    }
}

fn start_decks(case: &Case, decks: &[Deck], failures: &mut Vec<String>) {
    let deck_count = u16::try_from(case.media.len()).expect("matrix deck count fits u16");
    let level = MIX_HEADROOM / f32::from(deck_count);
    apply_mix(decks.iter().map(|deck| (deck.player.as_ref(), level)))
        .unwrap_or_else(|error| panic!("{}: apply conservative deck mix: {error}", case.label));

    for (deck_index, deck) in decks.iter().enumerate() {
        record_control_state(case, deck_index, deck, "before playback", failures);
        deck.player
            .select_item_with_crossfade(
                0,
                SelectTransition {
                    autoplay: true,
                    crossfade_seconds: 0.0,
                },
            )
            .unwrap_or_else(|error| {
                panic!("{} deck {deck_index}: select resource: {error}", case.label)
            });
    }
}

async fn prepare_deck(
    case: &Case,
    deck_index: usize,
    media: Media,
    server: &TestServerHelper,
    session: &Arc<OfflineSession>,
) -> Deck {
    let controls = StretchControls::new(1.0);
    controls.set_backend(StretchKind::Signalsmith);
    controls.set_keylock(true);
    let bus = EventBus::new(16_384);
    let dispatcher: Arc<dyn SessionDispatcher> = session.clone();
    let player = Arc::new(PlayerImpl::new(
        PlayerConfig::builder()
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .bus(bus)
            .sample_rate(case.host_rate)
            .crossfade_duration(0.0)
            .timestretch(Arc::clone(&controls))
            .session(dispatcher)
            .build(),
    ));
    let events = player.subscribe();
    let src = match media {
        Media::Mp3(name) => media_path(name)
            .to_str()
            .expect("repository media path is UTF-8")
            .to_owned(),
        Media::Hls => server.asset("hls/master.m3u8").to_string(),
    };
    let config =
        ResourceConfig::for_src(ResourceConfig::parse_src(&src).unwrap_or_else(|error| {
            panic!("{} deck {deck_index}: parse {src}: {error}", case.label)
        }))
        .store(memory_asset_store())
        .byte_pool(player.byte_pool().clone())
        .pcm_pool(player.pcm_pool().clone())
        .initial_abr_mode(AbrMode::manual(0))
        .discriminator(format!("{}-deck-{deck_index}", case.label))
        .build();
    let mut resource = time::timeout(
        PRELOAD_TIMEOUT,
        Resource::new(player.prepare_config(config)),
    )
    .await
    .unwrap_or_else(|_| panic!("{} deck {deck_index}: resource open timed out", case.label))
    .unwrap_or_else(|error| panic!("{} deck {deck_index}: open resource: {error}", case.label));
    time::timeout(PRELOAD_TIMEOUT, resource.preload())
        .await
        .unwrap_or_else(|_| panic!("{} deck {deck_index}: preload timed out", case.label))
        .unwrap_or_else(|error| {
            panic!(
                "{} deck {deck_index}: preload resource: {error}",
                case.label
            )
        });
    player.insert(
        resource,
        Some(Arc::from(format!("{}-deck-{deck_index}", case.label))),
        None,
    );

    Deck {
        player,
        controls,
        events,
        observation: DeckObservation {
            hls: matches!(media, Media::Hls),
            label: media.label(),
            ..DeckObservation::default()
        },
    }
}

async fn warm_to_steady_state(
    case: &Case,
    session: &OfflineSession,
    decks: &mut [Deck],
    failures: &mut Vec<String>,
) {
    let max_blocks = blocks_for_secs(case.host_rate, MAX_WARMUP_SECS);
    let mut active_streak = 0usize;
    for _ in 0..max_blocks {
        let block = render_paced(session, decks, case.host_rate).await;
        drain_all_events(decks, "warmup", failures);
        if block.len() == BLOCK_FRAMES * usize::from(CHANNELS)
            && block.iter().all(|sample| sample.is_finite())
            && rms(&block) >= ACTIVE_RMS
            && decks.iter().all(|deck| {
                deck.player.position_seconds().unwrap_or(0.0) >= MIN_WARMUP_POSITION_SECS
            })
        {
            active_streak += 1;
            if active_streak >= 2 {
                return;
            }
        } else {
            active_streak = 0;
        }
    }
    failures.push(format!(
        "{}: playback did not reach two consecutive active callback blocks within {MAX_WARMUP_SECS}s; positions={:?}",
        case.label,
        decks
            .iter()
            .map(|deck| deck.player.position_seconds())
            .collect::<Vec<_>>(),
    ));
}

async fn render_paced(session: &OfflineSession, decks: &[Deck], sample_rate: u32) -> Vec<f32> {
    for deck in decks {
        deck.player.process_notifications();
    }
    let block = session.render(BLOCK_FRAMES);
    time::sleep(Duration::from_secs_f64(
        f64::from(u32::try_from(BLOCK_FRAMES).expect("block frames fit u32"))
            / f64::from(sample_rate),
    ))
    .await;
    block
}

fn inspect_block(
    case: &Case,
    block_index: usize,
    block: &[f32],
    silent_blocks: &mut Vec<usize>,
    failures: &mut Vec<String>,
) {
    let expected = BLOCK_FRAMES * usize::from(CHANNELS);
    if block.len() != expected {
        failures.push(format!(
            "{} callback {block_index}: produced {} samples, expected {expected}",
            case.label,
            block.len(),
        ));
    }
    if block.iter().any(|sample| !sample.is_finite()) {
        failures.push(format!(
            "{} callback {block_index}: contained non-finite PCM",
            case.label,
        ));
    } else if rms(block) < ACTIVE_RMS {
        silent_blocks.push(block_index);
    }
}

fn drain_all_events(decks: &mut [Deck], phase: &str, failures: &mut Vec<String>) {
    for (deck_index, deck) in decks.iter_mut().enumerate() {
        loop {
            match deck.events.try_recv() {
                Ok(envelope) => match envelope.event {
                    Event::Audio(AudioEvent::UnderrunStarted { .. }) => failures.push(format!(
                        "deck {deck_index} ({}) reported an underrun during {phase}",
                        deck.observation.label,
                    )),
                    Event::Audio(AudioEvent::TrackFailed { failure, .. }) => failures.push(format!(
                        "deck {deck_index} ({}) reported track failure {failure:?} during {phase}",
                        deck.observation.label,
                    )),
                    Event::Audio(AudioEvent::PlaybackResamplerConfigured {
                        backend,
                        host_sample_rate,
                        source_sample_rate,
                        active,
                    }) => deck.observation.playback_resamplers.push(ResamplerObservation {
                        active,
                        backend: resampler_name(backend),
                        host_sample_rate,
                        source_sample_rate,
                    }),
                    Event::Decoder(DecoderEvent::DecoderChanged {
                        sample_rate,
                        channels,
                        variant,
                        ..
                    }) => {
                        deck.observation.decoder_changes += 1;
                        deck.observation.decoder_sample_rates.push(sample_rate);
                        deck.observation.decoder_channels.push(channels);
                        deck.observation.decoder_variants.push(variant);
                    }
                    Event::Decoder(DecoderEvent::DecodeError {
                        class,
                        kind,
                        detail,
                        ..
                    }) => failures.push(format!(
                        "deck {deck_index} ({}) decode error {class:?}/{kind:?} ({detail}) during {phase}",
                        deck.observation.label,
                    )),
                    Event::Player(PlayerEvent::ItemDidPlayToEnd { src, .. }) => failures.push(
                        format!("deck {deck_index} ({src}) reached player EOF during {phase}"),
                    ),
                    Event::Player(PlayerEvent::ItemDidFail { src, .. }) => failures.push(format!(
                        "deck {deck_index} ({src}) reported player track failure during {phase}",
                    )),
                    Event::Bus(BusEvent::Overflow { dropped, .. }) => failures.push(format!(
                        "deck {deck_index} ({}) event bus dropped {dropped} events during {phase}",
                        deck.observation.label,
                    )),
                    Event::Hls(HlsEvent::Error { error }) => failures.push(format!(
                        "deck {deck_index} ({}) HLS error {error:?} during {phase}",
                        deck.observation.label,
                    )),
                    Event::File(FileEvent::Error { error }) => failures.push(format!(
                        "deck {deck_index} ({}) file error {error:?} during {phase}",
                        deck.observation.label,
                    )),
                    Event::Downloader(DownloaderEvent::RequestFailed { error, .. }) => {
                        failures.push(format!(
                            "deck {deck_index} ({}) downloader request failed with {error:?} during {phase}",
                            deck.observation.label,
                        ));
                    }
                    Event::Downloader(DownloaderEvent::RetryExhausted { error, .. }) => {
                        failures.push(format!(
                            "deck {deck_index} ({}) downloader exhausted retries with {error:?} during {phase}",
                            deck.observation.label,
                        ));
                    }
                    Event::Item(ItemEvent::PlaybackStalled) => failures.push(format!(
                        "deck {deck_index} ({}) playback stalled during {phase}",
                        deck.observation.label,
                    )),
                    Event::Transport(event) => failures.push(format!(
                        "deck {deck_index} ({}) emitted unexpected no-SYNC transport event {event:?} during {phase}",
                        deck.observation.label,
                    )),
                    _ => {}
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(count)) => failures.push(format!(
                    "deck {deck_index} ({}) event receiver lost {count} events during {phase}",
                    deck.observation.label,
                )),
                Err(TryRecvError::Closed) => {
                    failures.push(format!(
                        "deck {deck_index} ({}) event receiver closed during {phase}",
                        deck.observation.label,
                    ));
                    break;
                }
            }
        }
    }
}

fn validate_deck(case: &Case, deck_index: usize, deck: &Deck, failures: &mut Vec<String>) {
    if deck.observation.decoder_changes != 1 {
        failures.push(format!(
            "{} deck {deck_index} ({}): observed {} decoder changes, expected one initial decoder",
            case.label, deck.observation.label, deck.observation.decoder_changes,
        ));
    }
    if deck.observation.decoder_sample_rates != [case.host_rate] {
        failures.push(format!(
            "{} deck {deck_index} ({}): decoder output rates {:?}, expected [{}]",
            case.label,
            deck.observation.label,
            deck.observation.decoder_sample_rates,
            case.host_rate,
        ));
    }
    if deck.observation.decoder_channels != [CHANNELS] {
        failures.push(format!(
            "{} deck {deck_index} ({}): decoder channels {:?}, expected [{CHANNELS}]",
            case.label, deck.observation.label, deck.observation.decoder_channels,
        ));
    }
    let expected_variants = if deck.observation.hls {
        vec![Some(0)]
    } else {
        vec![None]
    };
    if deck.observation.decoder_variants != expected_variants {
        failures.push(format!(
            "{} deck {deck_index} ({}): decoder variants {:?}, expected {:?}",
            case.label,
            deck.observation.label,
            deck.observation.decoder_variants,
            expected_variants,
        ));
    }
    if deck.observation.playback_resamplers.is_empty() {
        failures.push(format!(
            "{} deck {deck_index} ({}): no host-rate resampler event was observed",
            case.label, deck.observation.label,
        ));
    }
    let expected_active = case.host_rate != SOURCE_RATE;
    for resampler in &deck.observation.playback_resamplers {
        if resampler.host_sample_rate != case.host_rate
            || resampler.source_sample_rate != SOURCE_RATE
            || resampler.active != expected_active
        {
            failures.push(format!(
                "{} deck {deck_index} ({}): resampler {} reported source={} host={} active={}, expected source={SOURCE_RATE} host={} active={expected_active}",
                case.label,
                deck.observation.label,
                resampler.backend,
                resampler.source_sample_rate,
                resampler.host_sample_rate,
                resampler.active,
                case.host_rate,
            ));
        }
    }
    record_control_state(case, deck_index, deck, "after capture", failures);
}

fn record_control_state(
    case: &Case,
    deck_index: usize,
    deck: &Deck,
    phase: &str,
    failures: &mut Vec<String>,
) {
    if deck.controls.speed() != 1.0
        || deck.controls.region_plan().is_some()
        || deck.controls.backend() != StretchKind::Signalsmith
        || !deck.controls.keylock()
    {
        failures.push(format!(
            "{} deck {deck_index} ({}): invalid no-SYNC controls {phase} (speed={}, plan={}, backend={:?}, keylock={})",
            case.label,
            deck.observation.label,
            deck.controls.speed(),
            deck.controls.region_plan().is_some(),
            deck.controls.backend(),
            deck.controls.keylock(),
        ));
    }
}

fn record_transport_state(session: &OfflineSession, phase: &str, failures: &mut Vec<String>) {
    match session.exec(Cmd::QuerySessionTransport) {
        Ok(Reply::Err(SessionError::TransportNotProcessed)) => {}
        Ok(Reply::Err(error)) => failures.push(format!(
            "session transport returned {error} {phase}, expected unconfigured",
        )),
        Ok(_) => failures.push(format!(
            "session transport was configured {phase}, but this is a no-SYNC matrix",
        )),
        Err(error) => failures.push(format!("session transport query failed {phase}: {error}")),
    }
}

const fn resampler_name(kind: PlaybackResamplerKind) -> &'static str {
    match kind {
        PlaybackResamplerKind::Rubato => "rubato",
        PlaybackResamplerKind::Glide => "glide",
        PlaybackResamplerKind::None => "none",
        _ => "unknown",
    }
}

fn blocks_for_secs(sample_rate: u32, seconds: u32) -> usize {
    let frames = u64::from(sample_rate) * u64::from(seconds);
    usize::try_from(frames)
        .expect("matrix duration fits usize")
        .div_ceil(BLOCK_FRAMES)
}

// Music has no time-aligned clean replay in this matrix, so this cannot prove
// arbitrary in-band fidelity. It detects exact dropout/repeated buffers and
// callback-boundary jumps; the synthetic byte-exact test owns wider distortion.
fn measure_sample_continuity(samples: &[f32]) -> SampleContinuityReport {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    let mut longest_zero = 0usize;
    let mut zero_run = 0usize;
    for frame in samples[..frames * channels].chunks_exact(channels) {
        if frame.iter().all(|sample| *sample == 0.0) {
            zero_run += 1;
            longest_zero = longest_zero.max(zero_run);
        } else {
            zero_run = 0;
        }
    }

    let mut adjacent = Vec::with_capacity(frames.saturating_sub(1) * channels);
    for frame in 1..frames {
        for channel in 0..channels {
            let before = samples[(frame - 1) * channels + channel];
            let after = samples[frame * channels + channel];
            adjacent.push((after - before).abs());
        }
    }
    adjacent.sort_by(f32::total_cmp);
    let p99_adjacent_jump = adjacent
        .get(adjacent.len().saturating_sub(1) * 99 / 100)
        .copied()
        .unwrap_or(0.0);
    let boundary_limit = MIN_BOUNDARY_JUMP.max(p99_adjacent_jump * BOUNDARY_OUTLIER_RATIO);

    let mut discontinuities = Vec::new();
    let mut repeated = Vec::new();
    let mut max_boundary_jump = 0.0_f32;
    for boundary in (BLOCK_FRAMES..frames).step_by(BLOCK_FRAMES) {
        let jump = (0..channels)
            .map(|channel| {
                let before = samples[(boundary - 1) * channels + channel];
                let after = samples[boundary * channels + channel];
                (after - before).abs()
            })
            .fold(0.0_f32, f32::max);
        max_boundary_jump = max_boundary_jump.max(jump);
        if jump > boundary_limit {
            discontinuities.push(boundary);
        }

        let previous = (boundary - BLOCK_FRAMES) * channels..boundary * channels;
        let current = boundary * channels..(boundary + BLOCK_FRAMES).min(frames) * channels;
        if current.len() == previous.len() && samples[previous] == samples[current] {
            repeated.push(boundary);
        }
    }

    SampleContinuityReport {
        discontinuity_boundaries: discontinuities,
        longest_exact_zero_run_frames: longest_zero,
        max_boundary_jump,
        p99_adjacent_jump,
        repeated_block_boundaries: repeated,
    }
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / f64::from(u32::try_from(samples.len()).expect("callback samples fit u32"));
    power.sqrt()
}

fn media_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root above tests crate")
        .join("assets")
        .join(name)
}
