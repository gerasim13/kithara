#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    audio::mock::TestPcmReader,
    effects::LimiterConfig,
    host::{HostConfig, Tap},
    play::{PlayError, Tempo},
    signal::AudioSpec,
    warp::{Beat, BeatGridQuery, BeatGridSnapshot, BeatOrdinal, MapPoint, MapPosition},
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    bufpool_ext::{TestPools, pools},
    offline::{
        OfflineHostHarness, OfflinePlayer, OfflinePlayerOptions, TapProbe, resource_from_reader,
    },
};
use kithara_test_fixtures::{
    analysis_beat_fixtures::sine_440_long, integration_fixtures::constant_half,
};

use super::mix_tap::{
    ROOMY_CAPACITY, play_constant, play_resource, playing_harness, render_blocks,
};

mod consts {
    pub(super) const SAMPLE_RATE: u32 = 44_100;
    /// Twice [`SAMPLE_RATE`]: a route restart between the two rates halves
    /// or doubles the frame grid the Host beats round to.
    pub(super) const DOUBLE_RATE: u32 = 88_200;
    pub(super) const BLOCK_FRAMES: u32 = 512;
    pub(super) const CHANNELS: u16 = 2;
    /// A tempo whose beat is not a whole number of frames, so every click
    /// lands on a frame the anchor rounds to.
    pub(super) const BPM: f64 = 124.0;
    pub(super) const BLOCKS: u64 = 700;
    pub(super) const BEATS_PER_BAR: i64 = 4;
    /// Peak of a downbeat click at the default metronome level.
    pub(super) const DOWNBEAT_PEAK: f32 = 0.5;
    /// Peak of a beat click: five eighths of a downbeat.
    pub(super) const BEAT_PEAK: f32 = 0.3125;
    pub(super) const PEAK_TOLERANCE: f32 = 1e-6;
    /// How far under its level a click's sampled peak may fall: the samples
    /// nearest the crests of the tone around the envelope's peak miss the
    /// crest by less than this at every rate from 44.1 kHz.
    pub(super) const PEAK_SHORTFALL: f32 = 0.01;
    /// A click rises from silence: its beat frame carries the silent foot of
    /// the rise, so it first sounds one frame later.
    pub(super) const SILENT_FOOT: u64 = 1;
    /// Frames of one click at [`SAMPLE_RATE`]: its 2 ms rise and 8 ms fall.
    pub(super) const CLICK_FRAMES: u64 = 441;
    /// Blocks rendered after a pause so the deck's fade-out has settled.
    pub(super) const SETTLE_BLOCKS: usize = 4;
    /// Blocks rendered with every deck paused: more than two beats at [`BPM`].
    pub(super) const PAUSED_BLOCKS: usize = 100;
    /// Blocks rendered over a loud mix with one click in them.
    pub(super) const LOUD_BLOCKS: usize = 8;
    pub(super) const LOUD_CEILING: f32 = 0.25;
    pub(super) const LOUD_LEVEL: f32 = 0.2;
    /// The shallowest duck [`LOUD_LEVEL`] allows under [`LOUD_CEILING`]: the
    /// ducked mix plus the click meets the ceiling exactly.
    pub(super) const LOUD_DUCK: f32 = 0.8;
    /// Half of [`LOUD_DUCK`]: too shallow to keep a click at [`LOUD_LEVEL`]
    /// under [`LOUD_CEILING`].
    pub(super) const SHALLOW_DUCK: f32 = 0.4;
    pub(super) const TOO_LOUD_LEVEL: f32 = 0.5;
    /// The Host tempo a ride starts from.
    pub(super) const RIDE_FROM_BPM: u32 = 120;
    /// The Host tempo a ride ends at.
    pub(super) const RIDE_TO_BPM: u32 = 145;
    /// Beats a ride holds at each end: two bars.
    pub(super) const HOLD_BEATS: usize = 8;
    /// Blocks a tempo change may take to reach the published grid.
    pub(super) const GRID_WAIT_BLOCKS: usize = 4;
    /// Blocks rendered after a ride's last beat so its click ends in the take.
    pub(super) const CLICK_TAIL_BLOCKS: usize = 2;
    /// The louder of two decks that differ in nothing else.
    pub(super) const DECK_LOUD: f32 = 0.5;
    /// The softer of two decks that differ in nothing else.
    pub(super) const DECK_SOFT: f32 = 0.25;
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

/// Every click in `heard` peaks at the level of the Host beat it rises from:
/// a downbeat's or a beat's.
fn assert_click_levels(heard: &[Click], beats: &[(u64, bool)]) {
    for (click, (_, downbeat)) in heard.iter().zip(beats) {
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

/// The first Host beat of `grid` at or after `frame`, with whether it opens a
/// bar.
fn beat_from(grid: &BeatGridSnapshot, frame: u64) -> (u64, bool) {
    (0_i64..)
        .map(|ordinal| {
            (
                beat_frame(grid, ordinal),
                ordinal % consts::BEATS_PER_BAR == 0,
            )
        })
        .find(|(beat, _)| *beat >= frame)
        .expect("the Host grid runs past every frame")
}

/// The Host tempo of every beat of a ride: two bars at
/// [`consts::RIDE_FROM_BPM`], one BPM more on each beat up to
/// [`consts::RIDE_TO_BPM`], two bars there.
fn ride() -> Vec<u32> {
    let hold = |bpm| std::iter::repeat_n(bpm, consts::HOLD_BEATS);
    hold(consts::RIDE_FROM_BPM)
        .chain(consts::RIDE_FROM_BPM + 1..=consts::RIDE_TO_BPM)
        .chain(hold(consts::RIDE_TO_BPM))
        .collect()
}

fn peak(pcm: &[f32]) -> f32 {
    pcm.iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn capacity(frames: u64) -> usize {
    usize::try_from(frames).expect("tap capacity") * usize::from(consts::CHANNELS)
}

fn tempo() -> Tempo {
    Tempo::new(consts::BPM).expect("fixture tempo")
}

/// An offline Host at `sample_rate` with no deck, its metronome on, its
/// transport running at [`consts::BPM`], and an output tap holding `frames`
/// frames. One block is rendered first: the tempo commits on a running session.
async fn metronome_host(
    sample_rate: u32,
    frames: u64,
) -> (OfflineHostHarness<TestPools>, TapProbe) {
    let sample_rate = NonZeroU32::new(sample_rate).expect("test sample rate");
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
    let (host, mut tap) = metronome_host(consts::SAMPLE_RATE, frames).await;

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
    assert_click_levels(&heard, &beats);
}

#[kithara::test(tokio)]
async fn a_click_a_route_restart_interrupts_ends_when_it_would_have_at_the_new_rate() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let (host, mut tap) = metronome_host(consts::SAMPLE_RATE, consts::BLOCKS * block).await;
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
    host.set_sample_rate(NonZeroU32::new(consts::DOUBLE_RATE).expect("restart rate"))
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
    let scale = f64::from(consts::DOUBLE_RATE) / f64::from(consts::SAMPLE_RATE);
    let expected = (span - head_frames) as f64 * scale;
    assert_eq!(tail.frame, 0, "the click carries on across the restart");
    assert!(
        (tail.frames as f64 - expected).abs() <= scale,
        "the click ends when it would have: {} frames at the new rate, expected {expected:.0}",
        tail.frames
    );
}

/// Renders `host` up to `past` frames after Host beat 1, restarts the route at
/// `rate`, and returns the clicks the output tap sounds in the blocks after
/// the restart.
async fn clicks_after_a_restart_near_beat_one(
    host: OfflineHostHarness<TestPools>,
    mut tap: TapProbe,
    past: u64,
    rate: u32,
) -> Vec<Click> {
    let block = u64::from(consts::BLOCK_FRAMES);
    host.render_forward(block).await;
    let beat_one = beat_frame(&host.session_grid().await, 1);
    host.render_forward(beat_one + past - host.position()).await;
    tap.drain();
    host.set_sample_rate(NonZeroU32::new(rate).expect("restart rate"))
        .await
        .expect("restart the route at the new rate");
    host.render_forward(4 * block).await;
    let tail = clicks(&tap.drain());
    host.close().await;
    tail
}

#[kithara::test(tokio)]
async fn a_beat_a_restart_to_a_lower_rate_lands_on_clicks_once() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let (host, tap) = metronome_host(consts::DOUBLE_RATE, consts::BLOCKS * block).await;
    // WHY: At [`consts::BPM`] beat 1 lies 0.42 frames after the frame it
    // rounds to at the double rate. A restart one frame later puts it 0.29
    // frames before the restart frame at the lower rate: onto which it rounds
    // again.
    let tail =
        clicks_after_a_restart_near_beat_one(host, tap, consts::SILENT_FOOT, consts::SAMPLE_RATE)
            .await;

    let [tail] = tail.as_slice() else {
        panic!("only beat 1's click sounds after the restart: {tail:?}");
    };
    assert_eq!(
        tail.frame, 0,
        "beat 1's click carries on across the restart instead of starting again"
    );
}

#[kithara::test(tokio)]
async fn a_beat_a_restart_to_a_higher_rate_lands_on_still_clicks() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let (host, tap) = metronome_host(consts::SAMPLE_RATE, consts::BLOCKS * block).await;
    // WHY: At [`consts::BPM`] beat 1 lies 0.29 frames before the frame it
    // rounds to, the restart frame; at the double rate that is 0.58 frames,
    // which rounds onto the frame before the restart.
    let tail = clicks_after_a_restart_near_beat_one(host, tap, 0, consts::DOUBLE_RATE).await;

    let [tail] = tail.as_slice() else {
        panic!("beat 1 clicks once after the restart: {tail:?}");
    };
    assert_eq!(
        tail.frame,
        consts::SILENT_FOOT,
        "beat 1's click rises from the first frame after the restart"
    );
}

#[kithara::test(tokio)]
async fn a_metronome_switched_back_on_clicks_from_the_next_beat() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let frames = consts::BLOCKS * block;
    let (host, mut tap) = metronome_host(consts::SAMPLE_RATE, frames).await;
    host.render_forward(block).await;
    let beat_three = beat_frame(&host.session_grid().await, 3);
    host.set_metronome(false).await.expect("metronome off");
    host.render_forward(beat_three + block - host.position())
        .await;
    tap.drain();
    let start = host.position();
    host.set_metronome(true).await.expect("metronome back on");
    host.render_forward(frames - start).await;
    let heard: Vec<u64> = clicks(&tap.drain())
        .iter()
        .map(|click| click.frame + start)
        .collect();
    let grid = host.session_grid().await;
    host.close().await;

    let beats: Vec<u64> = host_beats(&grid, start..frames)
        .into_iter()
        .map(|(frame, _)| frame + consts::SILENT_FOOT)
        .collect();
    assert!(!beats.is_empty(), "the render spans Host beats");
    assert_eq!(
        heard, beats,
        "the beats the metronome was off for stay silent; it clicks from the next beat on"
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
    let (host, mut output) = metronome_host(consts::SAMPLE_RATE, frames).await;
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

/// A tempo ride rendered over a deck: both taps, the session frame they
/// start on and every Host beat of the ride with whether it opens a bar.
struct Ride {
    output: Vec<f32>,
    master: Vec<f32>,
    start: u64,
    beats: Vec<(u64, bool)>,
}

/// Rides the Host tempo from [`consts::RIDE_FROM_BPM`] to
/// [`consts::RIDE_TO_BPM`] with the metronome on over a deck playing `tone`.
async fn ride_over(tone: Vec<f32>) -> Ride {
    let frames = u64::try_from(tone.len()).expect("tone length");
    let rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate");
    let harness = play_resource(
        OfflinePlayer::with_sample_rate(
            OfflinePlayerOptions::builder().build(),
            consts::SAMPLE_RATE,
        )
        .await,
        move || {
            resource_from_reader(TestPcmReader::with_samples(
                AudioSpec::new(consts::CHANNELS, rate),
                tone,
            ))
        },
    )
    .await;
    let host = harness.host();
    let mut master = host
        .attach_tap(Tap::Master, capacity(frames))
        .await
        .expect("master tap");
    let mut output = host
        .attach_tap(Tap::Output, capacity(frames))
        .await
        .expect("output tap");
    host.set_metronome(true).await.expect("metronome on");
    let start = host.position();

    // WHY: A tempo change commits one block after it is set, onto a grid
    // retargeted from that frame. Setting it on the block boundary just after
    // a click sounds keeps every beat out of that window, so each beat is
    // placed by the grid of the step it belongs to.
    let mut beats = Vec::new();
    let mut bpm = None;
    for step in ride() {
        let from = host.position();
        if bpm != Some(step) {
            let revision = host.session_grid().await.revision();
            let tempo = Tempo::new(f64::from(step)).expect("ride tempo");
            host.with(move |host| host.set_tempo(tempo))
                .await
                .expect("Host tempo");
            let mut waited = 0;
            while host.session_grid().await.revision() == revision {
                assert!(
                    waited < consts::GRID_WAIT_BLOCKS,
                    "the Host publishes the {step} BPM grid"
                );
                render_blocks(&harness, 1).await;
                waited += 1;
            }
            bpm = Some(step);
        }
        let (beat, downbeat) = beat_from(&host.session_grid().await, from);
        while host.position() <= beat + consts::SILENT_FOOT {
            render_blocks(&harness, 1).await;
        }
        beats.push((beat, downbeat));
    }
    render_blocks(&harness, consts::CLICK_TAIL_BLOCKS).await;
    harness.close().await;

    assert_eq!(
        (output.drops(), master.drops()),
        (0, 0),
        "the taps keep every frame"
    );
    let ride = Ride {
        output: output.drain(),
        master: master.drain(),
        start,
        beats,
    };
    assert_eq!(
        ride.output.len(),
        ride.master.len(),
        "both taps see every rendered frame"
    );
    ride
}

#[kithara::test(tokio)]
async fn the_metronome_clicks_on_every_host_beat_over_a_deck_through_a_tempo_ride(
    sine_440_long: Vec<f32>,
) {
    let tone: Vec<f32> = sine_440_long
        .into_iter()
        .step_by(usize::from(consts::CHANNELS))
        .collect();
    let tone_peak = peak(&tone);
    let half_tone = tone.iter().map(|sample| sample / 2.0).collect();
    let full = ride_over(tone).await;
    let half = ride_over(half_tone).await;

    if let Some(mut artifact) =
        AudioArtifactTap::from_env(&artifact_label(), consts::SAMPLE_RATE, consts::CHANNELS)
            .expect("listening artifact")
    {
        artifact.push(&full.output);
    }
    assert_eq!(
        peak(&full.master),
        tone_peak,
        "the deck plays the tone at its own level under the metronome"
    );
    assert_eq!(
        (half.start, &half.beats),
        (full.start, &full.beats),
        "a quieter deck rides the same Host beats"
    );
    assert!(
        half.master
            .iter()
            .copied()
            .eq(full.master.iter().map(|sample| sample / 2.0)),
        "the deck at half its level mixes to exactly half the master"
    );
    // WHY: Each output frame is the frame's mix under the duck plus the
    // click, and both rides duck and click alike. Twice the half ride's
    // output less the full ride's cancels the mix and leaves the click.
    let clicked: Vec<f32> = half
        .output
        .iter()
        .zip(&full.output)
        .map(|(half, full)| half.mul_add(2.0, -full))
        .collect();
    let heard = clicks(&clicked);
    assert_eq!(
        heard
            .iter()
            .map(|click| click.frame + full.start)
            .collect::<Vec<_>>(),
        full.beats
            .iter()
            .map(|(frame, _)| frame + consts::SILENT_FOOT)
            .collect::<Vec<_>>(),
        "one click rises from every Host beat of the ride, and nowhere else"
    );
    let long = heard
        .iter()
        .find(|click| click.frames > consts::CLICK_FRAMES);
    assert!(long.is_none(), "no ride click outlasts one click: {long:?}");
    assert_click_levels(&heard, &full.beats);
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
        .metronome_duck(consts::LOUD_DUCK)
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
    let loudest = peak(&output);
    assert!(
        loudest <= consts::LOUD_CEILING * (1.0 + 4.0 * f32::EPSILON),
        "the ducked mix plus the click stays under the ceiling: {loudest}"
    );
    assert_eq!(
        output, rendered,
        "the output tap carries what graph_out plays"
    );
    assert_ne!(output, master.drain(), "a click sounds over the loud mix");
}

/// The output and master taps of a player at the default Host config whose
/// deck holds `level` while the metronome clicks at [`consts::BPM`] for
/// [`consts::PAUSED_BLOCKS`] blocks.
async fn deck_under_clicks(level: f32) -> (Vec<f32>, Vec<f32>) {
    let rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate");
    // WHY: One block plays before the taps attach and one more keeps the
    // deck sounding past the last rendered frame.
    let deck_blocks = u64::try_from(consts::PAUSED_BLOCKS + 2).expect("block count");
    let frames = u64::from(consts::BLOCK_FRAMES) * deck_blocks;
    let deck = vec![level; usize::try_from(frames).expect("deck length")];
    let harness = play_resource(
        OfflinePlayer::with_sample_rate(
            OfflinePlayerOptions::builder().build(),
            consts::SAMPLE_RATE,
        )
        .await,
        move || {
            resource_from_reader(TestPcmReader::with_samples(
                AudioSpec::new(consts::CHANNELS, rate),
                deck,
            ))
        },
    )
    .await;
    let host = harness.host();
    let mut master = host
        .attach_tap(Tap::Master, capacity(frames))
        .await
        .expect("master tap");
    let mut output = host
        .attach_tap(Tap::Output, capacity(frames))
        .await
        .expect("output tap");
    host.set_metronome(true).await.expect("metronome on");
    let tempo = tempo();
    host.with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    render_blocks(&harness, consts::PAUSED_BLOCKS).await;
    harness.close().await;
    assert_eq!(
        (output.drops(), master.drops()),
        (0, 0),
        "the taps keep every frame"
    );
    (output.drain(), master.drain())
}

#[kithara::test(tokio)]
async fn a_full_duck_mutes_the_deck_at_the_peak_of_every_click() {
    let (loud, loud_master) = deck_under_clicks(consts::DECK_LOUD).await;
    let (soft, soft_master) = deck_under_clicks(consts::DECK_SOFT).await;

    assert_eq!(
        (loud.len(), soft.len(), soft_master.len()),
        (loud_master.len(), loud_master.len(), loud_master.len()),
        "both runs render the same frames"
    );
    let channels = usize::from(consts::CHANNELS);
    // WHY: Both runs click the same beats with the same clicks, so where
    // their outputs agree while their decks differ, no deck reaches the
    // output: the duck's gain is zero there.
    let deck_muted: Vec<bool> = loud
        .chunks_exact(channels)
        .zip(soft.chunks_exact(channels))
        .zip(
            loud_master
                .chunks_exact(channels)
                .zip(soft_master.chunks_exact(channels)),
        )
        .map(|((loud, soft), (loud_master, soft_master))| {
            loud == soft && loud_master != soft_master
        })
        .collect();
    let clicked: Vec<f32> = loud
        .iter()
        .zip(&loud_master)
        .map(|(output, master)| output - master)
        .collect();
    let heard = clicks(&clicked);
    assert!(
        heard.len() > 1,
        "the render holds a downbeat click and a beat click: {heard:?}"
    );
    for click in &heard {
        let start = usize::try_from(click.frame).expect("click frame");
        let frames = usize::try_from(click.frames).expect("click length");
        assert!(
            deck_muted
                .iter()
                .skip(start)
                .take(frames)
                .any(|muted| *muted),
            "the click mutes the deck at its peak: {click:?}"
        );
    }
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

#[kithara::test(tokio)]
async fn a_metronome_duck_too_shallow_for_the_level_refuses_the_host() {
    let config = HostConfig::offline(pools())
        .limiter(
            LimiterConfig::builder()
                .ceiling(consts::LOUD_CEILING)
                .build()
                .expect("limiter ceiling"),
        )
        .metronome_level(consts::LOUD_LEVEL)
        .metronome_duck(consts::SHALLOW_DUCK)
        .build();
    match OfflineHostHarness::new(config).await {
        Err(PlayError::InvalidParameter { name, .. }) => assert_eq!(name, "metronome_duck"),
        Err(error) => panic!("a duck too shallow for the level is an invalid parameter: {error}"),
        Ok(_) => panic!("a duck too shallow for the level must refuse the Host"),
    }
}
