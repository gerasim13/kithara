use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use ::kithara::{
    events::{
        AudioEvent, DecoderEvent, Event, EventReceiver, PlayerEvent, QueueEvent, SeekLifecycleStage,
    },
    play::SessionTransportSnapshot,
};
use kithara_platform::{
    time::{self, Duration},
    tokio::sync::broadcast::error::TryRecvError,
};
use kithara_queue::{TrackId, TrackStatus};
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

use super::{
    AppTrace, CochleaObservation, PcmCapture, SelectedAnalysis, circular_phase, circular_spread,
    create_case_artifact_directory,
    fixture::{A, ARTIFACT_DIR_ENV, B, PHASE_BUDGET_FRAMES, TRACK_SECONDS},
    observe_cochlea,
    offline::{BLOCK_FRAMES, CHANNELS, SAMPLE_RATE},
    write_audio_manifest_entry, write_float_wav,
};
use crate::{beatmatch, deck::DeckId};

struct ColdScenario;

impl ColdScenario {
    const ANALYSIS_SECONDS: usize = 180;
    const ARTIFACT_CASE: &'static str = "app-cold-analysis-sync";
    const MAP_WAIT_SECONDS: u64 = 120;
    const SETTLE_SECONDS: usize = 4;
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    flash(false),
    timeout(Duration::from_secs(180))
)]
#[ignore = "manual cold-analysis Cochlea acceptance; run through the sync-acceptance workflow"]
async fn one_sync_request_survives_cold_analysis_and_converges_when_the_map_arrives() {
    let mut trace = AppTrace::new_cold_analysis().await;
    let warm_pcm = trace.warm_and_reset().await;
    let b_selected = trace
        .wait_for_selected_analysis(B, trace.b_track, 1, TRACK_SECONDS)
        .await;
    let host_bpm = beatmatch::deck_bpm(&b_selected.analysis)
        .expect("production analysis must expose the host deck tempo");

    trace.press_sync(B);
    let host_sync_visible = trace.wait_for_sync_mode(B).await;
    let host_transport = trace.app.session.session().transport().ok();
    let host_revision = host_transport
        .as_ref()
        .map(SessionTransportSnapshot::revision)
        .map(Into::into);

    trace.queue_a.play();
    let lifecycle_start_frame = trace.rendered_frames;
    let mut lifecycle_samples = trace.render_paced_block().await;
    let cold_at_request = trace
        .selected_analysis(A, trace.a_track, 0, ColdScenario::ANALYSIS_SECONDS)
        .is_none();
    let request_index = trace.queue_a.current_index();
    let request_track = trace.queue_a.current().map(|track| track.id);
    let request_position = trace.queue_a.position_seconds();
    trace.discard_events();

    let request_frame = trace.rendered_frames;
    trace.press_sync(A);
    let pending_after_request = trace.app.pending_sync;
    let light_after_request = trace.sync_light(A);
    let mut request_retained = pending_after_request == Some(A) && light_after_request;
    let mut facts = ColdEventFacts::default();
    let mut map_selected =
        trace.selected_analysis(A, trace.a_track, 0, ColdScenario::ANALYSIS_SECONDS);
    let mut map_ready_frame = map_selected.as_ref().map(|_| trace.rendered_frames);
    let map_wait_started = time::Instant::now();
    while map_selected.is_none()
        && map_wait_started.elapsed() < Duration::from_secs(ColdScenario::MAP_WAIT_SECONDS)
    {
        lifecycle_samples.extend(trace.render_paced_block().await);
        trace.capture_events(&mut facts);
        request_retained &= trace.app.pending_sync == Some(A) || trace.sync_light(A);
        map_selected = trace.selected_analysis(A, trace.a_track, 0, ColdScenario::ANALYSIS_SECONDS);
        if map_selected.is_some() {
            map_ready_frame = Some(trace.rendered_frames);
        }
    }

    let settle_frames = usize::try_from(SAMPLE_RATE)
        .expect("sample rate fits usize")
        .saturating_mul(ColdScenario::SETTLE_SECONDS);
    let settle_blocks = settle_frames.div_ceil(BLOCK_FRAMES);
    for _ in 0..settle_blocks {
        lifecycle_samples.extend(trace.render_paced_block().await);
        trace.capture_events(&mut facts);
        request_retained &= trace.app.pending_sync == Some(A) || trace.sync_light(A);
    }
    let result_frame = trace.rendered_frames;
    let converged = trace.app.pending_sync.is_none() && trace.sync_light(A);
    let result_transport = trace.app.session.session().transport().ok();
    let lifecycle = PcmCapture {
        samples: lifecycle_samples,
        start_frame: lifecycle_start_frame,
    };
    let result_index = trace.queue_a.current_index();
    let result_track = trace.queue_a.current().map(|track| track.id);
    let result_position = trace.queue_a.position_seconds();
    let failed_status = [&trace.queue_a, &trace.queue_b].into_iter().any(|queue| {
        queue
            .tracks()
            .iter()
            .any(|track| matches!(track.status, TrackStatus::Failed(_)))
    });

    let final_a = trace.capture_deck(A).await;
    let final_b = trace.capture_deck(B).await;
    let final_mix = trace.capture_mix().await;
    let captures = ColdSyncCaptures {
        lifecycle,
        final_a,
        final_b,
        final_mix,
    };
    let ledger = ColdFrameLedger {
        request: request_frame,
        map_ready: map_ready_frame,
        result: result_frame,
    };
    let observation = ColdSyncObservation {
        cold_at_request,
        converged,
        failed_status,
        host_bpm,
        host_revision,
        host_sync_visible,
        light_after_request,
        pending_after_request,
        request_index,
        request_position,
        request_retained,
        request_track,
        result_index,
        result_position,
        result_revision: result_transport
            .as_ref()
            .map(SessionTransportSnapshot::revision)
            .map(Into::into),
        result_track,
        sync_requests: 1,
        warm_pcm,
    };
    let cochlea = ColdCochleaObservations {
        lifecycle: observe_cochlea(&captures.lifecycle),
        final_a: observe_cochlea(&captures.final_a),
        final_b: observe_cochlea(&captures.final_b),
        final_mix: observe_cochlea(&captures.final_mix),
    };

    write_optional_cold_analysis_artifacts(
        &captures,
        &cochlea,
        &ledger,
        &facts,
        &observation,
        map_selected.as_ref(),
        &b_selected,
    );

    assert!(warm_pcm, "cold-analysis lifecycle must begin with real PCM");
    assert!(host_sync_visible, "deck B did not establish the host grid");
    assert!(
        cold_at_request,
        "the fixture did not issue SYNC before deck A's final beat map was ready"
    );
    assert_eq!(
        pending_after_request,
        Some(A),
        "a cold SYNC request must remain pending while analysis is unfinished"
    );
    assert!(
        light_after_request,
        "the SYNC light must retain the user's cold request"
    );
    assert!(
        request_retained,
        "the original SYNC request was lost before analysis could finish"
    );
    assert!(
        map_selected.is_some(),
        "deck A never published its final production beat map"
    );
    assert_eq!(observation.sync_requests, 1);
    assert!(
        converged,
        "deck A did not join the existing host grid when its map arrived; no second SYNC press was sent"
    );
    assert_eq!(
        request_index, result_index,
        "SYNC must not change queue index"
    );
    assert_eq!(
        request_track, result_track,
        "SYNC must not change current track"
    );
    assert_eq!(request_track, Some(trace.a_track));
    assert!(
        request_position
            .zip(result_position)
            .is_some_and(|(before, after)| after >= before),
        "cold analysis/SYNC must not rewind the source position: before={request_position:?}, after={result_position:?}"
    );
    assert_eq!(facts.current_track_changes, 0, "SYNC changed the track");
    assert_eq!(facts.decoder_reloads, 0, "SYNC reloaded a decoder");
    assert_eq!(facts.seek_requests, 0, "SYNC issued a source seek");
    assert_eq!(facts.underruns, 0, "cold analysis caused an underrun");
    assert_eq!(facts.failures, 0, "cold analysis caused a playback failure");
    assert!(!failed_status, "a fixture track entered Failed status");
    assert_eq!(observation.host_revision, observation.result_revision);
    assert!(
        captures
            .lifecycle
            .samples
            .iter()
            .all(|sample| sample.is_finite()),
        "cold-analysis PCM contains a non-finite sample"
    );
    assert!(
        captures
            .lifecycle
            .samples
            .iter()
            .any(|sample| sample.abs() > 0.01),
        "cold-analysis PCM is silent"
    );
    assert!(
        cochlea.lifecycle.clean,
        "cold-analysis lifecycle mix is not Cochlea-clean"
    );
    assert!(
        cochlea.final_a.clean && cochlea.final_b.clean && cochlea.final_mix.clean,
        "post-analysis Cochlea captures must not clip"
    );
    assert!(
        cochlea.final_a.clear_rhythm && cochlea.final_b.clear_rhythm,
        "post-analysis deck rhythm is not usable: A={:?}, B={:?}",
        cochlea.final_a.bpm,
        cochlea.final_b.bpm,
    );
    let final_a_bpm = cochlea
        .final_a
        .bpm
        .expect("rhythmic deck A observation carries BPM");
    let final_b_bpm = cochlea
        .final_b
        .bpm
        .expect("rhythmic deck B observation carries BPM");
    assert!(
        (final_a_bpm - host_bpm).abs() <= 1.0 && (final_b_bpm - host_bpm).abs() <= 1.0,
        "one retained request must converge both audible decks to {host_bpm:.3} BPM: A={:.3}, B={:.3}",
        final_a_bpm,
        final_b_bpm
    );
    let beat_period = (f64::from(SAMPLE_RATE) * 60.0 / host_bpm)
        .round()
        .to_u64()
        .expect("host beat period fits u64");
    let (a_phase, a_concentration) = circular_phase(&cochlea.final_a.beat_frames, beat_period)
        .expect("cold-analysis deck A beats must have a phase");
    let (b_phase, b_concentration) = circular_phase(&cochlea.final_b.beat_frames, beat_period)
        .expect("cold-analysis deck B beats must have a phase");
    assert!(
        a_concentration >= 0.5 && b_concentration >= 0.5,
        "post-analysis Cochlea phase must be stable: A={a_concentration:.3}, B={b_concentration:.3}"
    );
    let spread = circular_spread(&[a_phase, b_phase], beat_period)
        .expect("two post-analysis deck phases must produce a spread");
    assert!(
        spread <= PHASE_BUDGET_FRAMES,
        "post-analysis Cochlea beat spread is {spread} frames; budget is {PHASE_BUDGET_FRAMES}"
    );
}

struct ColdSyncCaptures {
    lifecycle: PcmCapture,
    final_a: PcmCapture,
    final_b: PcmCapture,
    final_mix: PcmCapture,
}

struct ColdCochleaObservations {
    lifecycle: CochleaObservation,
    final_a: CochleaObservation,
    final_b: CochleaObservation,
    final_mix: CochleaObservation,
}

struct ColdFrameLedger {
    request: i64,
    map_ready: Option<i64>,
    result: i64,
}

#[derive(Default)]
struct ColdEventFacts {
    current_track_changes: usize,
    decoder_reloads: usize,
    failures: usize,
    seek_requests: usize,
    underruns: usize,
}

struct ColdSyncObservation {
    cold_at_request: bool,
    converged: bool,
    failed_status: bool,
    host_bpm: f64,
    host_revision: Option<u64>,
    host_sync_visible: bool,
    light_after_request: bool,
    pending_after_request: Option<DeckId>,
    request_index: Option<usize>,
    request_position: Option<f64>,
    request_retained: bool,
    request_track: Option<TrackId>,
    result_index: Option<usize>,
    result_position: Option<f64>,
    result_revision: Option<u64>,
    result_track: Option<TrackId>,
    sync_requests: usize,
    warm_pcm: bool,
}

impl AppTrace {
    async fn new_cold_analysis() -> Self {
        Self::new_with_a_seconds(ColdScenario::ANALYSIS_SECONDS).await
    }

    async fn render_paced_block(&mut self) -> Vec<f32> {
        let cadence = Duration::from_secs_f64(
            BLOCK_FRAMES.to_f64().expect("block frame count fits f64") / f64::from(SAMPLE_RATE),
        );
        let started = time::Instant::now();
        let samples = self.render_frames(BLOCK_FRAMES).await;
        let elapsed = started.elapsed();
        if elapsed < cadence {
            time::sleep(cadence - elapsed).await;
        }
        samples
    }

    fn discard_events(&mut self) {
        let mut discarded = ColdEventFacts::default();
        self.capture_events(&mut discarded);
    }

    fn capture_events(&mut self, facts: &mut ColdEventFacts) {
        drain_cold_events(&mut self.events_a, facts);
        drain_cold_events(&mut self.events_b, facts);
    }
}

fn drain_cold_events(events: &mut EventReceiver, facts: &mut ColdEventFacts) {
    loop {
        match events.try_recv().map(|envelope| envelope.event) {
            Ok(Event::Audio(AudioEvent::UnderrunStarted { .. })) => {
                facts.underruns = facts.underruns.saturating_add(1);
            }
            Ok(Event::Audio(AudioEvent::SeekLifecycle {
                stage: SeekLifecycleStage::SeekRequest,
                ..
            })) => {
                facts.seek_requests = facts.seek_requests.saturating_add(1);
            }
            Ok(Event::Audio(AudioEvent::DecoderReady { .. }))
            | Ok(Event::Decoder(DecoderEvent::DecoderChanged { .. })) => {
                facts.decoder_reloads = facts.decoder_reloads.saturating_add(1);
            }
            Ok(Event::Audio(AudioEvent::TrackFailed { .. }))
            | Ok(Event::Decoder(DecoderEvent::DecodeError { .. }))
            | Ok(Event::Player(PlayerEvent::ItemDidFail { .. }))
            | Ok(Event::Queue(QueueEvent::TrackLoadFailed { .. }))
            | Ok(Event::Queue(QueueEvent::TrackStatusChanged {
                status: TrackStatus::Failed(_),
                ..
            })) => {
                facts.failures = facts.failures.saturating_add(1);
            }
            Ok(Event::Queue(QueueEvent::CurrentTrackChanged { .. })) => {
                facts.current_track_changes = facts.current_track_changes.saturating_add(1);
            }
            Ok(_) => {}
            Err(TryRecvError::Lagged(skipped)) => {
                facts.failures = facts
                    .failures
                    .saturating_add(usize::try_from(skipped).unwrap_or(usize::MAX));
            }
            Err(TryRecvError::Closed | TryRecvError::Empty) => break,
        }
    }
}

fn write_optional_cold_analysis_artifacts(
    captures: &ColdSyncCaptures,
    cochlea: &ColdCochleaObservations,
    ledger: &ColdFrameLedger,
    facts: &ColdEventFacts,
    observation: &ColdSyncObservation,
    map_selected: Option<&SelectedAnalysis>,
    host_selected: &SelectedAnalysis,
) {
    let Some(root) = std::env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return;
    };
    write_cold_analysis_artifact_bundle(
        &root,
        captures,
        cochlea,
        ledger,
        facts,
        observation,
        map_selected,
        host_selected,
    )
    .expect("write optional cold-analysis SYNC artifact bundle");
}

fn write_cold_analysis_artifact_bundle(
    root: &Path,
    captures: &ColdSyncCaptures,
    cochlea: &ColdCochleaObservations,
    ledger: &ColdFrameLedger,
    facts: &ColdEventFacts,
    observation: &ColdSyncObservation,
    map_selected: Option<&SelectedAnalysis>,
    host_selected: &SelectedAnalysis,
) -> io::Result<PathBuf> {
    fs::create_dir_all(root)?;
    let directory = create_case_artifact_directory(root, ColdScenario::ARTIFACT_CASE)?;
    let audio = [
        ("cold-analysis-lifecycle-mix.wav", &captures.lifecycle),
        ("post-analysis-deck-a.wav", &captures.final_a),
        ("post-analysis-deck-b.wav", &captures.final_b),
        ("post-analysis-mix.wav", &captures.final_mix),
    ];
    for (name, capture) in audio {
        write_float_wav(&directory.join(name), &capture.samples)?;
    }
    write_cold_analysis_manifest(
        &directory.join("manifest.json"),
        captures,
        cochlea,
        ledger,
        facts,
        observation,
        map_selected,
        host_selected,
    )?;
    Ok(directory)
}

fn write_cold_analysis_manifest(
    path: &Path,
    captures: &ColdSyncCaptures,
    cochlea: &ColdCochleaObservations,
    ledger: &ColdFrameLedger,
    facts: &ColdEventFacts,
    observation: &ColdSyncObservation,
    map_selected: Option<&SelectedAnalysis>,
    host_selected: &SelectedAnalysis,
) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    let map_ready = ledger
        .map_ready
        .map_or_else(|| "null".to_owned(), |frame| frame.to_string());
    let map_frames = map_selected.map(|selected| selected.analysis.source_frames());
    writeln!(writer, "{{")?;
    writeln!(writer, "  \"schema_version\": 1,")?;
    writeln!(writer, "  \"case\": \"{}\",", ColdScenario::ARTIFACT_CASE)?;
    writeln!(writer, "  \"sample_rate\": {SAMPLE_RATE},")?;
    writeln!(writer, "  \"channels\": {CHANNELS},")?;
    writeln!(writer, "  \"block_frames\": {BLOCK_FRAMES},")?;
    writeln!(
        writer,
        "  \"frame_ledger\": {{\"request\":{},\"map_ready\":{map_ready},\"result\":{}}},",
        ledger.request, ledger.result
    )?;
    writeln!(
        writer,
        "  \"analysis\": {{\"cold_at_request\":{},\"map_source_frames\":{},\"host_source_frames\":{},\"host_bpm\":{:.9}}},",
        observation.cold_at_request,
        json_option(map_frames),
        host_selected.analysis.source_frames(),
        observation.host_bpm,
    )?;
    writeln!(
        writer,
        "  \"request\": {{\"count\":{},\"pending_after_press\":{},\"light_after_press\":{},\"retained_until_map\":{}}},",
        observation.sync_requests,
        json_deck(observation.pending_after_request),
        observation.light_after_request,
        observation.request_retained,
    )?;
    writeln!(
        writer,
        "  \"result\": {{\"host_visible\":{},\"converged\":{},\"host_revision\":{},\"result_revision\":{}}},",
        observation.host_sync_visible,
        observation.converged,
        json_option(observation.host_revision),
        json_option(observation.result_revision),
    )?;
    writeln!(
        writer,
        "  \"identity\": {{\"request_index\":{},\"result_index\":{},\"request_track\":{},\"result_track\":{},\"request_position\":{},\"result_position\":{}}},",
        json_option(observation.request_index),
        json_option(observation.result_index),
        json_track(observation.request_track),
        json_track(observation.result_track),
        json_option(observation.request_position),
        json_option(observation.result_position),
    )?;
    writeln!(
        writer,
        "  \"continuity\": {{\"warm_pcm\":{},\"underruns\":{},\"seek_requests\":{},\"decoder_reloads\":{},\"current_track_changes\":{},\"failures\":{},\"failed_status\":{}}},",
        observation.warm_pcm,
        facts.underruns,
        facts.seek_requests,
        facts.decoder_reloads,
        facts.current_track_changes,
        facts.failures,
        observation.failed_status,
    )?;
    writeln!(
        writer,
        "  \"cochlea\": {{\"lifecycle_clean\":{},\"deck_a_clean\":{},\"deck_a_bpm\":{},\"deck_b_clean\":{},\"deck_b_bpm\":{},\"mix_clean\":{}}},",
        cochlea.lifecycle.clean,
        cochlea.final_a.clean,
        json_option(cochlea.final_a.bpm),
        cochlea.final_b.clean,
        json_option(cochlea.final_b.bpm),
        cochlea.final_mix.clean,
    )?;
    writeln!(writer, "  \"audio\": [")?;
    write_audio_manifest_entry(
        &mut writer,
        "cold_analysis_lifecycle_mix",
        "cold-analysis-lifecycle-mix.wav",
        &captures.lifecycle,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "post_analysis_deck_a",
        "post-analysis-deck-a.wav",
        &captures.final_a,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "post_analysis_deck_b",
        "post-analysis-deck-b.wav",
        &captures.final_b,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "post_analysis_mix",
        "post-analysis-mix.wav",
        &captures.final_mix,
        false,
    )?;
    writeln!(writer, "  ]")?;
    writeln!(writer, "}}")?;
    writer.flush()
}

fn json_option<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn json_track(value: Option<TrackId>) -> String {
    json_option(value.map(u64::from))
}

fn json_deck(value: Option<DeckId>) -> &'static str {
    match value {
        Some(A) => "\"A\"",
        Some(B) => "\"B\"",
        Some(_) | None => "null",
    }
}
