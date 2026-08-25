#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use kithara::{
    audio::{Audio, AudioConfig, AudioEffect, PcmSession},
    decode::PcmChunk,
    platform::time::{self, Duration},
    stream::Stream,
};
use kithara_integration_tests::{
    cochlea::{CochleaReport, assert_oracle_load_bearing, continuity_failures},
    create_test_wav, kithara,
    memory_source::{MemStream, MemStreamConfig, MemorySource},
    offline::{OfflinePlayer, resource_from_reader},
    sync_artifact::{
        ArtifactAudio, ArtifactFrame, ArtifactSource, SyncArtifactMetadata, write_sync_artifact,
    },
};
use num_traits::ToPrimitive;

use super::sync_latency::{RingDeck, SignalAsset, SyncFixture, analysis, sync_fixture_resources};

const BLOCK_FRAMES: usize = 512;
const CAPTURE_BLOCKS: usize = 188;
const CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 48_000;
const LOAD_BURST: Duration = Duration::from_millis(18);
const LOAD_INTERVAL_BLOCKS: usize = 8;
const PACED_CAPTURE_BLOCKS: usize = 96;
const PACED_WARMUP_BLOCKS: usize = 32;

struct BurstLoadEffect {
    blocks: usize,
    observed: SyncSender<()>,
}

impl BurstLoadEffect {
    fn new(observed: SyncSender<()>) -> Self {
        Self {
            blocks: 0,
            observed,
        }
    }
}

impl AudioEffect for BurstLoadEffect {
    fn flush(&mut self) -> Option<PcmChunk> {
        None
    }

    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        self.blocks = self.blocks.saturating_add(1);
        if self.blocks.is_multiple_of(LOAD_INTERVAL_BLOCKS) {
            let _ = self.observed.try_send(());
            let deadline = time::Instant::now() + LOAD_BURST;
            while time::Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        Some(chunk)
    }

    fn reset(&mut self) {
        self.blocks = 0;
    }
}

async fn load_player(
    deck: &RingDeck,
    resources: &kithara_integration_tests::sync_fixture::SyncFixtureResources,
) -> (OfflinePlayer, Receiver<()>) {
    let (observed_tx, observed_rx) = sync_channel(1);
    let stream = MemStreamConfig {
        source: Some(MemorySource::new(create_test_wav(
            SAMPLE_RATE as usize * 6,
            SAMPLE_RATE,
            CHANNELS,
        ))),
        event_bus: None,
    };
    let config = AudioConfig::<MemStream>::for_stream(stream)
        .byte_pool(resources.byte_pool().clone())
        .pcm_pool(resources.pcm_pool().clone())
        .worker(deck.worker())
        .effects(vec![Box::new(BurstLoadEffect::new(observed_tx))])
        .hint("wav".to_owned())
        .build();
    let mut audio = Audio::<Stream<MemStream>>::new(config)
        .await
        .expect("shared-worker load audio construction");
    let gate = audio
        .preload_gate()
        .expect("worker-backed load audio exposes preload gate");
    time::timeout(
        Duration::from_secs(5),
        gate.wait_for_epoch(audio.preload_epoch()),
    )
    .await
    .expect("shared-worker load audio preload gate opens");
    audio.preload().expect("shared-worker load audio preloads");
    let mut player = OfflinePlayer::new(SAMPLE_RATE);
    player.set_fade_duration(0.0);
    player.load_and_fadein(resource_from_reader(audio), "bound-sync-shared-worker-load");
    (player, observed_rx)
}

async fn capture_paced_bound(
    fixture: &SyncFixture,
    with_load: bool,
) -> (Vec<f32>, u64, bool, Vec<String>) {
    let deck = RingDeck::load(fixture.resources(), fixture.chirp(), BLOCK_FRAMES).await;
    let _ = deck.bind(&analysis(0));
    let (mut load, observed) = if with_load {
        let (player, observed) = load_player(&deck, deck.resources()).await;
        (Some(player), Some(observed))
    } else {
        (None, None)
    };
    let block_period = Duration::from_secs_f64(
        BLOCK_FRAMES.to_f64().expect("block size fits f64") / f64::from(SAMPLE_RATE),
    );
    for _ in 0..PACED_WARMUP_BLOCKS {
        let started = time::Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(BLOCK_FRAMES);
        }
        let _ = deck.pull().await;
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    if let Some(receiver) = observed.as_ref() {
        while receiver.try_recv().is_ok() {}
    }
    let underruns_before = deck.underruns();
    let mut pcm = Vec::with_capacity(PACED_CAPTURE_BLOCKS * BLOCK_FRAMES * usize::from(CHANNELS));
    for _ in 0..PACED_CAPTURE_BLOCKS {
        let started = time::Instant::now();
        if let Some(player) = load.as_mut() {
            let _ = player.render(BLOCK_FRAMES);
        }
        pcm.extend_from_slice(&deck.pull().await);
        time::sleep(block_period.saturating_sub(started.elapsed())).await;
    }
    let load_observed = observed.is_some_and(|receiver| receiver.try_recv().is_ok());
    (
        pcm,
        deck.underruns().saturating_sub(underruns_before),
        load_observed,
        deck.failures(),
    )
}

struct BoundCapture {
    failures: Vec<String>,
    pcm: Vec<f32>,
    start_frame: u64,
}

async fn capture_bound(
    fixture: &SyncFixture,
    signal: &SignalAsset,
    retarget_bpm: Option<f64>,
) -> BoundCapture {
    let deck = RingDeck::load(fixture.resources(), signal, BLOCK_FRAMES).await;
    let _ = deck.bind(&analysis(0));
    let _ = deck.capture_blocks(16).await;
    if let Some(bpm) = retarget_bpm {
        deck.set_session_tempo(bpm);
        let _ = deck.capture_blocks(16).await;
    }
    let start_frame = deck.committed_frames();
    let pcm = deck.capture_blocks(CAPTURE_BLOCKS).await;
    BoundCapture {
        failures: deck.failures(),
        pcm,
        start_frame,
    }
}

#[kithara::test(tokio, multi_thread, serial, timeout(Duration::from_secs(180)))]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-RT-001: waiting for Wave ResidentPlan"]
async fn bound_sync_render_is_rtsan_clean() {
    let fixture = SyncFixture::new(sync_fixture_resources("rt-bound-render")).await;
    let control = capture_bound(&fixture, fixture.chirp(), None).await;
    let candidate = capture_bound(&fixture, fixture.chirp(), Some(132.0)).await;
    let control_report = CochleaReport::measure(&control.pcm, CHANNELS, SAMPLE_RATE);
    let candidate_report = CochleaReport::measure(&candidate.pcm, CHANNELS, SAMPLE_RATE);
    let mut failures =
        continuity_failures("RTSan bound render", &candidate_report, &control_report);
    failures.extend(
        control
            .failures
            .iter()
            .map(|failure| format!("control: {failure}")),
    );
    failures.extend(candidate.failures.iter().cloned());
    if !candidate.pcm.iter().any(|sample| sample.abs() > 0.01) {
        failures.push("bound RTSan render produced no audible PCM".to_owned());
    }
    let mut metadata =
        SyncArtifactMetadata::new("bound-sync-rtsan", SAMPLE_RATE, CHANNELS, BLOCK_FRAMES);
    metadata.add_source(ArtifactSource::new("deck-a", fixture.chirp().uri()));
    metadata.set_operation("render a resident bound deck across a session-tempo retarget");
    metadata.add_frame(ArtifactFrame::new(
        control.start_frame,
        "control-capture-start",
    ));
    metadata.add_frame(ArtifactFrame::new(
        candidate.start_frame,
        "retarget-and-capture-start",
    ));
    metadata.add_threshold("candidate_extra_continuity_failures", 0.0);
    metadata.add_failures(failures.clone());
    write_sync_artifact(
        &metadata,
        &[
            ArtifactAudio::new("deck-a-stem", &candidate.pcm),
            ArtifactAudio::new("final-mix", &candidate.pcm),
            ArtifactAudio::new("pre-retarget-control", &control.pcm),
        ],
    )
    .expect("optional RTSan sync artifact writes before assertions");

    assert_oracle_load_bearing(&control.pcm, CHANNELS, SAMPLE_RATE, BLOCK_FRAMES);
    assert!(
        failures.is_empty(),
        "bound RTSan PCM contract failed: {}\ncontrol={control_report:?}\ncandidate={candidate_report:?}",
        failures.join("; "),
    );
}

#[kithara::test(
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(180))
)]
#[ignore = "SYNC-ORACLE SYNC-PRODUCT-RT-002: waiting for Wave ResidentPlan"]
async fn bound_sync_pcm_stays_clean_under_shared_worker_deadline_load() {
    let fixture = SyncFixture::new(sync_fixture_resources("rt-shared-worker-load")).await;
    let (control, control_underruns, _, control_failures) =
        capture_paced_bound(&fixture, false).await;
    let (candidate, candidate_underruns, load_observed, candidate_failures) =
        capture_paced_bound(&fixture, true).await;
    let control_report = CochleaReport::measure(&control, CHANNELS, SAMPLE_RATE);
    let candidate_report = CochleaReport::measure(&candidate, CHANNELS, SAMPLE_RATE);
    let mut failures = continuity_failures(
        "bound shared-worker load",
        &candidate_report,
        &control_report,
    );
    failures.extend(
        control_failures
            .iter()
            .map(|failure| format!("control: {failure}")),
    );
    failures.extend(candidate_failures);
    if control_underruns != 0 {
        failures.push(format!(
            "paced bound control reported {control_underruns} underrun transition(s)",
        ));
    }
    if candidate_underruns != 0 {
        failures.push(format!(
            "paced bound load reported {candidate_underruns} underrun transition(s)",
        ));
    }
    if !load_observed {
        failures.push("shared-worker deadline load did not run".to_owned());
    }

    let mut metadata = SyncArtifactMetadata::new(
        "bound-sync-shared-worker-deadline-load",
        SAMPLE_RATE,
        CHANNELS,
        BLOCK_FRAMES,
    );
    metadata.add_source(ArtifactSource::new("deck-a", fixture.chirp().uri()));
    metadata.add_source(ArtifactSource::new(
        "load",
        "generated-wav-18ms-worker-bursts",
    ));
    metadata.set_operation("pace an already-bound Queue/Player deck at 512-frame deadlines");
    metadata.add_state("control_underruns", control_underruns.to_string());
    metadata.add_state("candidate_underruns", candidate_underruns.to_string());
    metadata.add_threshold("extra_cochlea_failures", 0.0);
    metadata.add_failures(failures.clone());
    write_sync_artifact(
        &metadata,
        &[
            ArtifactAudio::new("deck-a-stem", &candidate),
            ArtifactAudio::new("final-mix", &candidate),
            ArtifactAudio::new("time-aligned-control", &control),
        ],
    )
    .expect("optional bound load artifact writes before assertions");

    assert_oracle_load_bearing(&control, CHANNELS, SAMPLE_RATE, BLOCK_FRAMES);
    assert!(
        failures.is_empty(),
        "bound shared-worker deadline contract failed: {}\ncontrol={control_report:?}\ncandidate={candidate_report:?}",
        failures.join("; "),
    );
}
