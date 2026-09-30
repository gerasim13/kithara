#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    effects::LimiterConfig,
    host::{HostConfig, Tap},
    play::{PlayError, Tempo},
    warp::{Beat, BeatGridQuery, BeatGridSnapshot, BeatOrdinal, MapPoint, MapPosition},
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    bufpool_ext::{TestPools, pools},
    offline::{OfflineHostHarness, OfflinePlayer, TapProbe},
};
use kithara_test_fixtures::integration_fixtures::constant_half;

use super::mix_tap::{ROOMY_CAPACITY, play_constant, playing_harness, render_blocks};

mod consts {
    pub(super) const SAMPLE_RATE: u32 = 44_100;
    /// The device rate a route restart moves the session to.
    pub(super) const RESTART_RATE: u32 = 88_200;
    pub(super) const BLOCK_FRAMES: u32 = 512;
    pub(super) const CHANNELS: u16 = 2;
    /// A tempo whose beat is not a whole number of frames, so every click
    /// lands on a frame the anchor rounds to.
    pub(super) const BPM: f64 = 124.0;
    pub(super) const BLOCKS: u64 = 700;
    pub(super) const BEATS_PER_BAR: i64 = 4;
    /// Peak of a downbeat click at the default metronome level.
    pub(super) const DOWNBEAT_PEAK: f32 = 0.144;
    /// Peak of a beat click: five eighths of a downbeat.
    pub(super) const BEAT_PEAK: f32 = 0.09;
    pub(super) const PEAK_TOLERANCE: f32 = 1e-6;
    /// How far under its level a click's sampled peak may fall: the samples
    /// nearest the crests of the tone around the envelope's peak miss the
    /// crest by less than this at every rate from 44.1 kHz.
    pub(super) const PEAK_SHORTFALL: f32 = 0.01;
    /// A click rises from silence: its beat frame carries the silent foot of
    /// the rise, so it first sounds one frame later.
    pub(super) const SILENT_FOOT: u64 = 1;
    /// Blocks rendered after a pause so the deck's fade-out has settled.
    pub(super) const SETTLE_BLOCKS: usize = 4;
    /// Blocks rendered with every deck paused: more than two beats at [`BPM`].
    pub(super) const PAUSED_BLOCKS: usize = 100;
    /// Blocks rendered over a loud mix with one click in them.
    pub(super) const LOUD_BLOCKS: usize = 8;
    pub(super) const LOUD_CEILING: f32 = 0.25;
    pub(super) const LOUD_LEVEL: f32 = 0.2;
    pub(super) const TOO_LOUD_LEVEL: f32 = 0.5;
}

/// One click the output tap sounded: the first frame it sounds on, how many
/// frames it sounds for and its peak.
#[derive(Debug)]
struct Click {
    frame: u64,
    frames: u64,
    peak: f32,
}

/// Every click in `pcm`. A click opens on a sounding frame after silence and
/// ends where two frames in a row are silent: its own waveform may cross
/// zero on a single sample, silence lasts longer.
fn clicks(pcm: &[f32]) -> Vec<Click> {
    let channels = usize::from(consts::CHANNELS);
    let mut found: Vec<Click> = Vec::new();
    let mut last_sounding: Option<u64> = None;
    for (frame, samples) in pcm.chunks_exact(channels).enumerate() {
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if peak == 0.0 {
            continue;
        }
        let frame = frame as u64;
        match found.last_mut() {
            Some(click) if last_sounding.is_some_and(|last| frame - last <= 2) => {
                click.frames = frame - click.frame + 1;
                click.peak = click.peak.max(peak);
            }
            _ => found.push(Click {
                frame,
                frames: 1,
                peak,
            }),
        }
        last_sounding = Some(frame);
    }
    found
}

/// The session frame of Host beat `ordinal`.
fn beat_frame(grid: &BeatGridSnapshot, ordinal: i64) -> u64 {
    let beat = Beat::try_from(BeatOrdinal::new(ordinal)).expect("whole Host beat");
    let BeatGridQuery::Resolved(position) = grid.position_at(MapPoint::new(grid.stamp(), beat))
    else {
        panic!("the Host grid places beat {ordinal}");
    };
    let MapPosition::Session(frame) = *position.value().value() else {
        panic!("the Host grid is on the session axis");
    };
    u64::try_from(i64::from(frame)).expect("Host beat after the session start")
}

/// The session frame of every whole Host beat inside `frames`, with whether
/// it opens a bar. Beat 0 is the tempo commit frame; there are no earlier beats.
fn host_beats(grid: &BeatGridSnapshot, frames: std::ops::Range<u64>) -> Vec<(u64, bool)> {
    (0_i64..)
        .map(|ordinal| {
            (
                beat_frame(grid, ordinal),
                ordinal % consts::BEATS_PER_BAR == 0,
            )
        })
        .take_while(|(frame, _)| *frame < frames.end)
        .filter(|(frame, _)| frames.contains(frame))
        .collect()
}

fn capacity(frames: u64) -> usize {
    usize::try_from(frames).expect("tap capacity") * usize::from(consts::CHANNELS)
}

fn tempo() -> Tempo {
    Tempo::new(consts::BPM).expect("fixture tempo")
}

/// An offline Host with no deck, its metronome on, its transport running at
/// [`consts::BPM`], and an output tap holding `frames` frames. One block is
/// rendered first: the tempo commits on a running session.
async fn metronome_host(frames: u64) -> (OfflineHostHarness<TestPools>, TapProbe) {
    let sample_rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate");
    let config = HostConfig::offline(pools())
        .sample_rate(sample_rate)
        .max_block_frames(NonZeroU32::new(consts::BLOCK_FRAMES).expect("test block size"))
        .build();
    let host = OfflineHostHarness::new(config)
        .await
        .expect("offline Host without a deck");
    let tap = host
        .attach_tap(Tap::Output, capacity(frames))
        .await
        .expect("output tap");
    host.set_metronome(true).await.expect("metronome on");
    host.render_forward(u64::from(consts::BLOCK_FRAMES)).await;
    let tempo = tempo();
    host.with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    (host, tap)
}

#[kithara::test(tokio)]
async fn the_engine_metronome_clicks_on_every_host_beat_with_no_deck_playing() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let frames = consts::BLOCKS * block;
    let (host, mut tap) = metronome_host(frames).await;

    let rendered = host.render_forward(frames - block).await;
    let pcm = tap.drain();
    let grid = host.session_grid().await;
    host.close().await;

    if let Some(mut artifact) =
        AudioArtifactTap::from_env(&artifact_label(), consts::SAMPLE_RATE, consts::CHANNELS)
            .expect("listening artifact")
    {
        artifact.push(&pcm);
    }
    assert_eq!(
        rendered,
        frames - block,
        "the Host renders every requested frame"
    );
    assert_eq!(tap.drops(), 0, "the tap keeps every frame");
    assert_eq!(
        pcm.len(),
        capacity(frames),
        "the tap sees the priming block too"
    );
    let beats = host_beats(&grid, 0..frames);
    let heard = clicks(&pcm);
    assert!(
        beats.len() >= 3 * usize::try_from(consts::BEATS_PER_BAR).expect("bar length"),
        "the render spans several bars: {beats:?}"
    );
    assert_eq!(
        heard.iter().map(|click| click.frame).collect::<Vec<_>>(),
        beats
            .iter()
            .map(|(frame, _)| frame + consts::SILENT_FOOT)
            .collect::<Vec<_>>(),
        "one click rises from the frame of every Host beat, and nowhere else"
    );
    for (click, (_, downbeat)) in heard.iter().zip(&beats) {
        let expected = if *downbeat {
            consts::DOWNBEAT_PEAK
        } else {
            consts::BEAT_PEAK
        };
        assert!(
            click.peak <= expected + consts::PEAK_TOLERANCE
                && click.peak >= expected * (1.0 - consts::PEAK_SHORTFALL),
            "a {} click peaks at {expected}: {click:?}",
            if *downbeat { "downbeat" } else { "beat" }
        );
    }
}

#[kithara::test(tokio)]
async fn a_click_a_route_restart_interrupts_ends_when_it_would_have_at_the_new_rate() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let (host, mut tap) = metronome_host(consts::BLOCKS * block).await;
    host.render_forward(block).await;
    let beat_one = beat_frame(&host.session_grid().await, 1);
    host.render_forward(beat_one - host.position()).await;
    let whole = clicks(&tap.drain());
    let [first] = whole.as_slice() else {
        panic!("one click sounds before beat 1: {whole:?}");
    };

    let span = first.frames + consts::SILENT_FOOT;
    let head_frames = span / 2;
    host.render_forward(head_frames).await;
    let head = clicks(&tap.drain());
    host.set_sample_rate(NonZeroU32::new(consts::RESTART_RATE).expect("restart rate"))
        .await
        .expect("restart the route at the new rate");
    host.render_forward(4 * first.frames).await;
    let tail = clicks(&tap.drain());
    host.close().await;

    assert_eq!(
        head.iter().map(|click| click.frames).collect::<Vec<_>>(),
        [head_frames - consts::SILENT_FOOT],
        "beat 1's click is sounding when the route restarts"
    );
    let [tail] = tail.as_slice() else {
        panic!("only the interrupted click sounds after the restart: {tail:?}");
    };
    let scale = f64::from(consts::RESTART_RATE) / f64::from(consts::SAMPLE_RATE);
    let expected = (span - head_frames) as f64 * scale;
    assert_eq!(tail.frame, 0, "the click carries on across the restart");
    assert!(
        (tail.frames as f64 - expected).abs() <= scale,
        "the click ends when it would have: {} frames at the new rate, expected {expected:.0}",
        tail.frames
    );
}

#[kithara::test(tokio)]
async fn the_metronome_is_off_by_default_and_passes_the_mix_bit_exactly(
    constant_half: &'static [u8],
) {
    let harness = playing_harness(constant_half).await;
    let mut master = harness
        .host()
        .attach_tap(Tap::Master, ROOMY_CAPACITY)
        .await
        .expect("master tap");
    let mut output = harness
        .host()
        .attach_tap(Tap::Output, ROOMY_CAPACITY)
        .await
        .expect("output tap");
    let tempo = tempo();
    harness
        .host()
        .with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    let rendered = render_blocks(&harness, 20).await;
    harness.close().await;

    let master = master.drain();
    assert_eq!(
        master, rendered,
        "the master tap carries graph_out while the metronome is off"
    );
    assert_eq!(
        output.drain(),
        master,
        "an off metronome passes the limited mix bit-exactly"
    );
}

#[kithara::test(tokio)]
async fn the_master_tap_carries_no_click() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let frames = 100 * block;
    let (host, mut output) = metronome_host(frames).await;
    let mut master = host
        .attach_tap(Tap::Master, capacity(frames))
        .await
        .expect("master tap");
    host.render_forward(frames - block).await;
    host.close().await;

    assert!(
        master.drain().iter().all(|sample| *sample == 0.0),
        "the master tap is the limited mix alone: silence with no deck"
    );
    assert!(!clicks(&output.drain()).is_empty(), "the output tap clicks");
}

#[kithara::test(tokio)]
async fn the_metronome_clicks_on_host_beats_while_every_deck_is_paused(
    constant_half: &'static [u8],
) {
    let harness = playing_harness(constant_half).await;
    harness.with_player(|player| player.pause()).await;
    render_blocks(&harness, consts::SETTLE_BLOCKS).await;

    let mut master = harness
        .host()
        .attach_tap(Tap::Master, ROOMY_CAPACITY * 2)
        .await
        .expect("master tap");
    let mut output = harness
        .host()
        .attach_tap(Tap::Output, ROOMY_CAPACITY * 2)
        .await
        .expect("output tap");
    harness
        .host()
        .set_metronome(true)
        .await
        .expect("metronome on");
    let tempo = tempo();
    harness
        .host()
        .with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");

    let start = harness.host().position();
    let rendered = render_blocks(&harness, consts::PAUSED_BLOCKS).await;
    let end = harness.host().position();
    let grid = harness.host().session_grid().await;
    harness.close().await;

    let output = output.drain();
    assert!(
        master.drain().iter().all(|sample| *sample == 0.0),
        "every deck is paused: the master tap is silent"
    );
    assert_eq!(
        output, rendered,
        "the output tap carries what graph_out plays"
    );
    let heard: Vec<u64> = clicks(&output)
        .iter()
        .map(|click| click.frame + start)
        .collect();
    let beats: Vec<u64> = host_beats(&grid, start..end)
        .into_iter()
        .map(|(frame, _)| frame + consts::SILENT_FOOT)
        .collect();
    assert!(!beats.is_empty(), "the render spans Host beats");
    assert_eq!(
        heard, beats,
        "the metronome follows the Host grid with no deck playing"
    );
}

#[kithara::test(tokio)]
async fn the_duck_under_a_click_keeps_a_loud_mix_at_or_under_the_limiter_ceiling(
    constant_half: &'static [u8],
) {
    let session = HostConfig::offline(pools())
        .sample_rate(NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate"))
        .limiter(
            LimiterConfig::builder()
                .ceiling(consts::LOUD_CEILING)
                .build()
                .expect("limiter ceiling"),
        )
        .metronome_level(consts::LOUD_LEVEL)
        .build();
    let harness = play_constant(OfflinePlayer::new(session).await, constant_half).await;
    let mut master = harness
        .host()
        .attach_tap(Tap::Master, ROOMY_CAPACITY)
        .await
        .expect("master tap");
    let mut output = harness
        .host()
        .attach_tap(Tap::Output, ROOMY_CAPACITY)
        .await
        .expect("output tap");
    harness
        .host()
        .set_metronome(true)
        .await
        .expect("metronome on");
    let tempo = tempo();
    harness
        .host()
        .with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    let rendered = render_blocks(&harness, consts::LOUD_BLOCKS).await;
    harness.close().await;

    let output = output.drain();
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        peak <= consts::LOUD_CEILING * (1.0 + 4.0 * f32::EPSILON),
        "the ducked mix plus the click stays under the ceiling: {peak}"
    );
    assert_eq!(
        output, rendered,
        "the output tap carries what graph_out plays"
    );
    assert_ne!(output, master.drain(), "a click sounds over the loud mix");
}

#[kithara::test(tokio)]
async fn a_metronome_level_above_the_limiter_ceiling_refuses_the_host() {
    let config = HostConfig::offline(pools())
        .limiter(
            LimiterConfig::builder()
                .ceiling(consts::LOUD_CEILING)
                .build()
                .expect("limiter ceiling"),
        )
        .metronome_level(consts::TOO_LOUD_LEVEL)
        .build();
    match OfflineHostHarness::new(config).await {
        Err(PlayError::InvalidParameter { name, .. }) => assert_eq!(name, "metronome_level"),
        Err(error) => panic!("a level over the ceiling is an invalid parameter: {error}"),
        Ok(_) => panic!("a level over the ceiling must refuse the Host"),
    }
}
