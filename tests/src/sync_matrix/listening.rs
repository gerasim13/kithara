use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use kithara::play::apply_mix;

use super::{
    CHANNELS, Operation, PcmCapture, RENDER_FRAMES, SyncCase, SyncHarness, media::SyntheticFixture,
};
use crate::{audio_artifact::write_audio_dump, sync_fixture::SyncFixtureResources};

/// Capture the five PR #150 listening files through Player/Queue and the final session mix.
pub async fn write_sync_listening_dump(
    resources: SyncFixtureResources,
    steady_case: SyncCase,
    sweep_case: SyncCase,
    ride_to_bpm: f64,
    sweep_to_bpm: f64,
    ride_steps: usize,
) -> Result<Option<PathBuf>> {
    if steady_case.sample_rate != sweep_case.sample_rate {
        bail!("listening cases must share one output sample rate");
    }
    if ride_steps == 0 {
        bail!("listening tempo ride must contain at least one step");
    }
    let steady_fixture = SyntheticFixture::new(steady_case, resources.clone()).await?;
    let steady_media = steady_fixture.media();
    let deck_a = capture_solo(steady_case, steady_media.clone(), 0).await?;
    let deck_b = capture_solo(steady_case, steady_media.clone(), 1).await?;
    let fixed_mix = capture_fixed_mix(steady_case, steady_media.clone()).await?;
    let ridden_mix = capture_ridden_mix(steady_case, steady_media, ride_to_bpm, ride_steps).await?;

    let sweep_fixture = SyntheticFixture::new(sweep_case, resources.clone()).await?;
    let swept_mix =
        capture_ridden_mix(sweep_case, sweep_fixture.media(), sweep_to_bpm, ride_steps).await?;
    let audio = [
        ("01_deck_a_96bpm_sine", deck_a.samples.as_slice()),
        ("02_deck_b_128bpm_square", deck_b.samples.as_slice()),
        ("03_mix_on_a_120bpm_grid", fixed_mix.samples.as_slice()),
        ("04_mix_riding_120_to_126", ridden_mix.samples.as_slice()),
        ("05_mix_sweeping_90_to_145", swept_mix.samples.as_slice()),
    ];
    write_audio_dump("kithara_mix", steady_case.sample_rate, CHANNELS, &audio)
        .context("write legacy sync listening WAVs")
}

async fn capture_solo(
    case: SyncCase,
    media: super::SyncMedia,
    audible_deck: usize,
) -> Result<PcmCapture> {
    let mut harness = SyncHarness::open(case, media).await?;
    apply_mix(harness.decks.iter().enumerate().map(|(index, deck)| {
        (
            deck.player.as_ref(),
            if index == audible_deck { 1.0 } else { 0.0 },
        )
    }))
    .with_context(|| format!("{case}: solo deck {audible_deck}"))?;
    play_all(&mut harness).await?;
    capture_frames(&mut harness, "solo", case.capture_frames()).await
}

async fn capture_fixed_mix(case: SyncCase, media: super::SyncMedia) -> Result<PcmCapture> {
    let mut harness = SyncHarness::open(case, media).await?;
    start_synced_mix(&mut harness).await?;
    capture_frames(&mut harness, "mix", case.capture_frames()).await
}

async fn capture_ridden_mix(
    case: SyncCase,
    media: super::SyncMedia,
    to_bpm: f64,
    steps: usize,
) -> Result<PcmCapture> {
    let mut harness = SyncHarness::open(case, media).await?;
    start_synced_mix(&mut harness).await?;
    let frames = case.capture_frames();
    let (start_session_frame, mut tap) = harness.start_pcm_capture()?;
    let mut rendered = 0;
    for step in 1..=steps {
        let progress = step as f64 / steps as f64;
        let bpm = (to_bpm - case.session_bpm).mul_add(progress, case.session_bpm);
        if !harness.attempt_ride_tempo(bpm)? {
            bail!(
                "{}: tempo ride at {bpm:.6} BPM was not accepted",
                harness.case
            );
        }
        let deadline = frames * step / steps;
        harness.render_frames(deadline - rendered).await?;
        rendered = deadline;
    }
    let capture = harness.finish_pcm_capture("mix", start_session_frame, &mut tap);
    if !harness.capture_failures.is_empty() {
        bail!(
            "{}: listening capture failed:\n{}",
            harness.case,
            harness.capture_failures.join("\n")
        );
    }
    Ok(capture)
}

async fn start_synced_mix(harness: &mut SyncHarness) -> Result<()> {
    apply_mix(harness.decks.iter().map(|deck| (deck.player.as_ref(), 0.5)))
        .with_context(|| format!("{}: set legacy mix gains", harness.case))?;
    play_all(harness).await?;
    for deck in 0..harness.decks.len() {
        harness.apply(deck, Operation::Sync)?;
        harness.render_frames(RENDER_FRAMES * 4).await?;
    }
    Ok(())
}

async fn play_all(harness: &mut SyncHarness) -> Result<()> {
    for deck in 0..harness.decks.len() {
        harness.apply(deck, Operation::Play)?;
    }
    harness.wait_all_playing().await
}

async fn capture_frames(
    harness: &mut SyncHarness,
    label: &str,
    frames: usize,
) -> Result<PcmCapture> {
    let (start_session_frame, mut tap) = harness.start_pcm_capture()?;
    harness.render_frames(frames).await?;
    let capture = harness.finish_pcm_capture(label, start_session_frame, &mut tap);
    if !harness.capture_failures.is_empty() {
        bail!(
            "{}: listening capture failed:\n{}",
            harness.case,
            harness.capture_failures.join("\n")
        );
    }
    Ok(capture)
}
