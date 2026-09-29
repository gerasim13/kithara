#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    host::{HostConfig, Tap},
    play::Tempo,
    warp::{Beat, BeatGridQuery, BeatGridSnapshot, BeatOrdinal, MapPoint, MapPosition},
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    bufpool_ext::pools,
    offline::OfflineHostHarness,
};

mod consts {
    pub(super) const SAMPLE_RATE: u32 = 44_100;
    pub(super) const BLOCK_FRAMES: u32 = 512;
    pub(super) const CHANNELS: u16 = 2;
    /// A tempo whose beat is not a whole number of frames, so every click
    /// lands on a frame the anchor rounds to.
    pub(super) const BPM: f64 = 124.0;
    pub(super) const BLOCKS: u64 = 700;
    pub(super) const SECONDS_PER_MINUTE: f64 = 60.0;
    pub(super) const BEATS_PER_BAR: i64 = 4;
}

/// One click the metronome tap sounded: the frame it starts on and its peak.
#[derive(Debug)]
struct Click {
    frame: u64,
    peak: f32,
}

/// Every click in `pcm`: a click opens on a sounding frame more than half a
/// beat after the last click opened, and holds every sounding frame until the
/// next one opens. A click's own waveform may cross zero on a sample.
fn clicks(pcm: &[f32]) -> Vec<Click> {
    let channels = usize::from(consts::CHANNELS);
    let half_beat = f64::from(consts::SAMPLE_RATE) * consts::SECONDS_PER_MINUTE / consts::BPM / 2.0;
    let mut found: Vec<Click> = Vec::new();
    for (frame, samples) in pcm.chunks_exact(channels).enumerate() {
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if peak == 0.0 {
            continue;
        }
        let frame = frame as u64;
        match found.last_mut() {
            Some(last) if ((frame - last.frame) as f64) <= half_beat => {
                last.peak = last.peak.max(peak);
            }
            _ => found.push(Click { frame, peak }),
        }
    }
    found
}

/// The session frame of every whole Host beat inside `frames`, with whether
/// it opens a bar.
fn host_beats(grid: &BeatGridSnapshot, frames: std::ops::Range<u64>) -> Vec<(u64, bool)> {
    (0..)
        .map_while(|ordinal: i64| {
            let beat = Beat::try_from(BeatOrdinal::new(ordinal)).expect("whole Host beat");
            let BeatGridQuery::Resolved(position) =
                grid.position_at(MapPoint::new(grid.stamp(), beat))
            else {
                panic!("the Host grid places beat {ordinal}");
            };
            let MapPosition::Session(frame) = *position.value().value() else {
                panic!("the Host grid is on the session axis");
            };
            let frame = u64::try_from(i64::from(frame)).expect("Host beat after the session start");
            frames
                .contains(&frame)
                .then_some((frame, ordinal % consts::BEATS_PER_BAR == 0))
        })
        .collect()
}

#[kithara::test(tokio)]
async fn the_engine_metronome_clicks_on_every_host_beat_with_no_deck_playing() {
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
    let frames = consts::BLOCKS * u64::from(consts::BLOCK_FRAMES);
    let mut tap = host
        .attach_tap(
            Tap::Metronome,
            usize::try_from(frames).expect("tap capacity") * usize::from(consts::CHANNELS),
        )
        .await
        .expect("metronome tap");

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
