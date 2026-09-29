#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    host::{HostConfig, Tap},
    play::Tempo,
    warp::{Beat, BeatGridQuery, BeatGridSnapshot, BeatOrdinal, MapPoint, MapPosition},
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    bufpool_ext::{TestPools, pools},
    offline::{OfflineHostHarness, TapProbe},
};

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
}

/// One click the metronome tap sounded: the frame it starts on, how many
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
/// it opens a bar.
fn host_beats(grid: &BeatGridSnapshot, frames: std::ops::Range<u64>) -> Vec<(u64, bool)> {
    (0..)
        .map_while(|ordinal: i64| {
            let frame = beat_frame(grid, ordinal);
            frames
                .contains(&frame)
                .then_some((frame, ordinal % consts::BEATS_PER_BAR == 0))
        })
        .collect()
}

/// An offline Host with no deck, its transport running at [`consts::BPM`],
/// and a tap on its metronome holding `frames` frames.
async fn metronome_host(frames: u64) -> (OfflineHostHarness<TestPools>, TapProbe) {
    let sample_rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate");
    let config = HostConfig::offline(pools())
        .sample_rate(sample_rate)
        .max_block_frames(NonZeroU32::new(consts::BLOCK_FRAMES).expect("test block size"))
        .build();
    let host = OfflineHostHarness::new(config)
        .await
        .expect("offline Host without a deck");
    let tempo = Tempo::new(consts::BPM).expect("fixture tempo");
    host.with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    let tap = host
        .attach_tap(
            Tap::Metronome,
            usize::try_from(frames).expect("tap capacity") * usize::from(consts::CHANNELS),
        )
        .await
        .expect("metronome tap");
    (host, tap)
}

#[kithara::test(tokio)]
async fn the_engine_metronome_clicks_on_every_host_beat_with_no_deck_playing() {
    let frames = consts::BLOCKS * u64::from(consts::BLOCK_FRAMES);
    let (host, mut tap) = metronome_host(frames).await;

    let rendered = host.render_forward(frames).await;
    let pcm = tap.drain();
    let grid = host.session_grid().await;
    host.close().await;

    if let Some(mut artifact) =
        AudioArtifactTap::from_env(&artifact_label(), consts::SAMPLE_RATE, consts::CHANNELS)
            .expect("listening artifact")
    {
        artifact.push(&pcm);
    }
    assert_eq!(rendered, frames, "the Host renders every requested frame");
    assert_eq!(tap.drops(), 0, "the tap keeps every metronome frame");
    let beats = host_beats(&grid, 0..frames);
    let heard = clicks(&pcm);
    assert!(
        beats.len() >= 3 * usize::try_from(consts::BEATS_PER_BAR).expect("bar length"),
        "the render spans several bars: {beats:?}"
    );
    assert_eq!(
        heard.iter().map(|click| click.frame).collect::<Vec<_>>(),
        beats.iter().map(|(frame, _)| *frame).collect::<Vec<_>>(),
        "one click starts on the frame of every Host beat, and nowhere else"
    );
    let (downbeats, beats): (Vec<_>, Vec<_>) = heard
        .iter()
        .zip(&beats)
        .partition(|(_, (_, downbeat))| *downbeat);
    let quietest_downbeat = downbeats
        .iter()
        .map(|(click, _)| click.peak)
        .fold(f32::INFINITY, f32::min);
    let loudest_beat = beats
        .iter()
        .map(|(click, _)| click.peak)
        .fold(0.0, f32::max);
    assert!(
        quietest_downbeat > loudest_beat,
        "each downbeat click is accented above every other beat: {downbeats:?} vs {beats:?}"
    );
}

#[kithara::test(tokio)]
async fn a_click_a_route_restart_interrupts_ends_when_it_would_have_at_the_new_rate() {
    let block = u64::from(consts::BLOCK_FRAMES);
    let (host, mut tap) = metronome_host(consts::BLOCKS * block).await;
    host.render_forward(block).await;
    let beat_one = beat_frame(&host.session_grid().await, 1);
    host.render_forward(beat_one - block).await;
    let whole = clicks(&tap.drain());
    let [first] = whole.as_slice() else {
        panic!("one click sounds before beat 1: {whole:?}");
    };

    let head_frames = first.frames / 2;
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
        [head_frames],
        "beat 1's click is sounding when the route restarts"
    );
    let [tail] = tail.as_slice() else {
        panic!("only the interrupted click sounds after the restart: {tail:?}");
    };
    let scale = f64::from(consts::RESTART_RATE) / f64::from(consts::SAMPLE_RATE);
    let expected = (first.frames - head_frames) as f64 * scale;
    assert_eq!(tail.frame, 0, "the click carries on across the restart");
    assert!(
        (tail.frames as f64 - expected).abs() <= scale,
        "the click ends when it would have: {} frames at the new rate, expected {expected:.0}",
        tail.frames
    );
}
