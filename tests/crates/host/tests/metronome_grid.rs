#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    audio::mock::TestPcmReader,
    host::{HostConfig, MetronomeConfig, Tap},
    play::Tempo,
    signal::AudioSpec,
    warp::BeatGridSnapshot,
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    bufpool_ext::pools,
    offline::{OfflineHostHarness, OfflinePlayer, resource_from_reader},
};
use kithara_test_fixtures::unit_fixtures::warp_nominal_clicks_long;

use super::{
    metronome::{assert_click_levels, beat_frame, capacity, clicks},
    mix_tap::{play_resource, render_blocks},
};

mod consts {
    pub(super) const SAMPLE_RATE: u32 = 44_100;
    pub(super) const BLOCK_FRAMES: u32 = 512;
    pub(super) const CHANNELS: u16 = 2;
    pub(super) const BEATS_PER_BAR: u32 = 4;
    /// The Host eases a tempo change in: the tempo approaches the new one as
    /// `e^(-t/τ)` with this time constant, in seconds.
    pub(super) const TEMPO_SMOOTH_SECONDS: f64 = 0.005;
    /// A tempo change commits one render block after it is set: the block
    /// the audio thread may already be rendering. The first tempo commits on
    /// the frame it is set on, since no grid plays before it.
    pub(super) const CHANGE_LEAD_FRAMES: u64 = 512;
    /// The Host tempo a ride starts from.
    pub(super) const RIDE_FROM_BPM: u32 = 120;
    /// The Host tempo a ride ends at, one BPM more on each change.
    pub(super) const RIDE_TO_BPM: u32 = 145;
    /// Blocks each ride tempo holds: under two beats, so the changes land on
    /// ever different phases of a beat.
    pub(super) const STEP_BLOCKS: u64 = 77;
    /// Blocks the last ride tempo holds: over eight beats.
    pub(super) const LAST_BLOCKS: u64 = 308;
    /// The Host places a beat on the frame nearest its exact time: half a
    /// frame, with slack for the float noise of two computations.
    pub(super) const NEAREST_FRAME: f64 = 0.5 + 1e-9;
    /// A beat this close to a tempo commit could be placed on either side of
    /// it, and this close to the end of a take could click past it: the ride
    /// keeps every beat further away.
    pub(super) const BOUNDARY_FRAMES: f64 = 2.0;
    /// Halvings that pin an exact beat frame far below float resolution.
    pub(super) const BISECTION_STEPS: usize = 128;
    /// A click's first sounding frame follows its beat frame: the click
    /// starts from silence.
    pub(super) const SILENT_FOOT: u64 = 1;
    /// The default metronome level: a downbeat click at the limiter ceiling.
    pub(super) const FULL_LEVEL: f32 = 1.0;
    /// The tempo of the known-tempo fragment: 22 050 frames a beat.
    pub(super) const KNOWN_BPM: u32 = 120;
    /// Frames a beat lasts at [`KNOWN_BPM`] and [`SAMPLE_RATE`].
    pub(super) const KNOWN_PERIOD: u64 = 22_050;
    /// Beats the known-tempo fragment clicks: eight bars.
    pub(super) const KNOWN_BEATS: u64 = 32;
    /// The fragment's clicks at this share sit under the limiter ceiling, so
    /// the master is the deck and a louder take is exactly twice a softer.
    pub(super) const FRAGMENT_LEVEL: f32 = 0.5;
    /// Metronome level and duck of a take over the fragment: the fragment's
    /// clicks stay audible under the metronome's.
    pub(super) const TAKE_METRONOME: f32 = 0.5;
    /// Blocks rendered past the fragment so its last click and the
    /// metronome's end in the take.
    pub(super) const TAIL_BLOCKS: usize = 16;
}

/// One stretch of the Host tempo as the math has it. From `frame` on, the
/// tempo eases from `from` to `to` beats per second as
/// `to + (from - to)·e^(-t/τ)` and the beat grows from `beat` by its
/// integral.
#[derive(Clone, Copy, Debug)]
struct Stretch {
    frame: f64,
    beat: f64,
    from: f64,
    to: f64,
}

impl Stretch {
    /// The first tempo, set on `frame` with no tempo before it: beat 0.
    fn first(frame: u64, bpm: u32) -> Self {
        let tempo = beats_per_second(bpm);
        Self {
            frame: exact(frame),
            beat: 0.0,
            from: tempo,
            to: tempo,
        }
    }

    /// The stretch a change to `bpm` committed on `frame` starts: it goes on
    /// from the beat and the tempo this one reached there.
    fn then(self, frame: u64, bpm: u32) -> Self {
        let frame = exact(frame);
        Self {
            frame,
            beat: self.beat_at(frame),
            from: self.tempo_at(frame),
            to: beats_per_second(bpm),
        }
    }

    fn seconds(self, frame: f64) -> f64 {
        (frame - self.frame) / f64::from(consts::SAMPLE_RATE)
    }

    fn tempo_at(self, frame: f64) -> f64 {
        let eased = (-self.seconds(frame) / consts::TEMPO_SMOOTH_SECONDS).exp();
        self.to + (self.from - self.to) * eased
    }

    fn beat_at(self, frame: f64) -> f64 {
        let seconds = self.seconds(frame);
        let eased =
            -consts::TEMPO_SMOOTH_SECONDS * (-seconds / consts::TEMPO_SMOOTH_SECONDS).exp_m1();
        self.beat + self.to * seconds + (self.from - self.to) * eased
    }

    /// The exact, fractional frame this stretch reaches `beat` on. The beat
    /// grows at least as fast as the slower of the two tempos, which bounds
    /// the search.
    fn frame_of(self, beat: f64) -> f64 {
        let slowest = self.from.min(self.to);
        let mut lower = self.frame;
        let mut upper =
            self.frame + (beat - self.beat) / slowest * f64::from(consts::SAMPLE_RATE) + 1.0;
        for _ in 0..consts::BISECTION_STEPS {
            let middle = lower.midpoint(upper);
            if self.beat_at(middle) < beat {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        lower.midpoint(upper)
    }
}

/// A whole beat the math places: its Host ordinal, its exact frame and the
/// stretch whose tempo places it.
#[derive(Clone, Copy, Debug)]
struct ComputedBeat {
    ordinal: u32,
    frame: f64,
    stretch: usize,
}

impl ComputedBeat {
    fn downbeat(self) -> bool {
        self.ordinal % consts::BEATS_PER_BAR == 0
    }
}

fn beats_per_second(bpm: u32) -> f64 {
    f64::from(bpm) / 60.0
}

fn exact(frame: u64) -> f64 {
    f64::from(u32::try_from(frame).expect("a take fits u32 frames"))
}

/// Every whole beat the math places before `end`, stretch by stretch.
fn computed_beats(stretches: &[Stretch], end: u64) -> Vec<ComputedBeat> {
    let end = exact(end);
    let mut beats = Vec::new();
    let mut ordinal = 0_u32;
    for (index, stretch) in stretches.iter().enumerate() {
        let until = stretches.get(index + 1).map_or(end, |next| next.frame);
        loop {
            let frame = stretch.frame_of(f64::from(ordinal));
            if frame >= until {
                break;
            }
            beats.push(ComputedBeat {
                ordinal,
                frame,
                stretch: index,
            });
            ordinal += 1;
        }
    }
    let boundaries: Vec<f64> = stretches
        .iter()
        .skip(1)
        .map(|stretch| stretch.frame)
        .chain(std::iter::once(end))
        .collect();
    let crowded = beats.iter().find(|beat| {
        boundaries
            .iter()
            .any(|boundary| (beat.frame - boundary).abs() < consts::BOUNDARY_FRAMES)
    });
    assert!(
        crowded.is_none(),
        "every beat sits clear of a tempo commit and of the take's end: {crowded:?}"
    );
    beats
}

/// One tempo of a ride: its BPM, the frame it was set on and the grid the
/// Host published for it.
struct RideStep {
    bpm: u32,
    requested: u64,
    grid: BeatGridSnapshot,
}

/// A tempo ride the Host rendered with no deck: its output tap from frame
/// `start`, every tempo it was given and the frame the ride ended on.
struct HostRide {
    output: Vec<f32>,
    start: u64,
    end: u64,
    steps: Vec<RideStep>,
}

impl HostRide {
    /// The stretches the math places the ride's beats with: the first tempo
    /// commits on its own frame, each change one lead later.
    fn stretches(&self) -> Vec<Stretch> {
        let mut stretches: Vec<Stretch> = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let stretch = stretches.last().map_or_else(
                || Stretch::first(step.requested, step.bpm),
                |last| last.then(step.requested + consts::CHANGE_LEAD_FRAMES, step.bpm),
            );
            stretches.push(stretch);
        }
        stretches
    }
}

/// Rides an offline Host with its metronome on and no deck from
/// [`consts::RIDE_FROM_BPM`] to [`consts::RIDE_TO_BPM`], one BPM a step.
async fn host_ride() -> HostRide {
    let block = u64::from(consts::BLOCK_FRAMES);
    let steps = u64::from(consts::RIDE_TO_BPM - consts::RIDE_FROM_BPM);
    let frames = block * (1 + steps * consts::STEP_BLOCKS + consts::LAST_BLOCKS);
    let config = HostConfig::offline(pools())
        .sample_rate(NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate"))
        .max_block_frames(NonZeroU32::new(consts::BLOCK_FRAMES).expect("test block size"))
        .build();
    let host = OfflineHostHarness::new(config)
        .await
        .expect("offline Host without a deck");
    let mut tap = host
        .attach_tap(Tap::Output, capacity(frames))
        .await
        .expect("output tap");
    host.set_metronome(true).await.expect("metronome on");
    let start = host.position();
    host.render_forward(block).await;

    let mut ride = Vec::new();
    for bpm in consts::RIDE_FROM_BPM..=consts::RIDE_TO_BPM {
        let revision = host.session_grid().await.revision();
        let requested = host.position();
        let tempo = Tempo::new(f64::from(bpm)).expect("ride tempo");
        host.with(move |host| host.set_tempo(tempo))
            .await
            .expect("Host tempo");
        let commit = if ride.is_empty() {
            requested
        } else {
            requested + consts::CHANGE_LEAD_FRAMES
        };
        host.render_forward(commit + block - host.position()).await;
        assert_ne!(
            host.session_grid().await.revision(),
            revision,
            "the Host publishes the {bpm} BPM grid once the block its commit lands in renders"
        );
        ride.push(RideStep {
            bpm,
            requested,
            grid: host.session_grid().await,
        });
        let hold = if bpm == consts::RIDE_TO_BPM {
            consts::LAST_BLOCKS
        } else {
            consts::STEP_BLOCKS
        };
        let until = requested + hold * block;
        host.render_forward(until - host.position()).await;
    }
    let end = host.position();
    host.close().await;

    assert_eq!(tap.drops(), 0, "the tap keeps every frame");
    HostRide {
        output: tap.drain(),
        start,
        end,
        steps: ride,
    }
}

#[kithara::test(tokio)]
async fn the_host_grid_places_every_beat_on_the_frame_nearest_its_computed_time() {
    let ride = host_ride().await;
    let beats = computed_beats(&ride.stretches(), ride.end);

    let first = &ride.steps[0];
    assert_eq!(
        beat_frame(&first.grid, 0),
        first.requested,
        "the first tempo pins beat 0 on the frame it is set on"
    );
    assert!(
        beats.len() > ride.steps.len(),
        "the ride spans more beats than tempo changes: {}",
        beats.len()
    );
    // WHY: Each published grid covers the stretch from its own commit on;
    // a beat is read from the grid of the stretch the math places it in.
    let misplaced: Vec<(u32, u64, f64)> = beats
        .iter()
        .filter_map(|beat| {
            let host = beat_frame(&ride.steps[beat.stretch].grid, i64::from(beat.ordinal));
            ((exact(host) - beat.frame).abs() > consts::NEAREST_FRAME).then_some((
                beat.ordinal,
                host,
                beat.frame,
            ))
        })
        .collect();
    assert!(
        misplaced.is_empty(),
        "the Host grid places every beat on the frame nearest its computed time \
         (ordinal, Host frame, computed frame): {misplaced:?}"
    );
}

#[kithara::test(tokio)]
async fn the_metronome_clicks_on_the_frame_nearest_every_computed_beat() {
    let ride = host_ride().await;
    let beats = computed_beats(&ride.stretches(), ride.end);

    if let Some(mut artifact) =
        AudioArtifactTap::from_env(&artifact_label(), consts::SAMPLE_RATE, consts::CHANNELS)
            .expect("listening artifact")
    {
        artifact.push(&ride.output);
    }
    let heard = clicks(&ride.output);
    assert_eq!(
        heard.len(),
        beats.len(),
        "one click sounds for every computed beat, and no other"
    );
    let off: Vec<(u32, u64, f64)> = heard
        .iter()
        .zip(&beats)
        .filter_map(|(click, beat)| {
            let placed = click.frame + ride.start - consts::SILENT_FOOT;
            ((exact(placed) - beat.frame).abs() > consts::NEAREST_FRAME).then_some((
                beat.ordinal,
                placed,
                beat.frame,
            ))
        })
        .collect();
    assert!(
        off.is_empty(),
        "every click rises from the frame nearest its computed beat \
         (ordinal, click beat frame, computed frame): {off:?}"
    );
    let accents: Vec<(u64, bool)> = heard
        .iter()
        .zip(&beats)
        .map(|(click, beat)| (click.frame, beat.downbeat()))
        .collect();
    assert_click_levels(&heard, &accents, consts::FULL_LEVEL);
}

/// A take of a deck playing the known-tempo fragment at `level` under the
/// Host metronome at [`consts::KNOWN_BPM`]: both taps from frame `start` and
/// the frame the tempo was set on.
struct KnownTake {
    output: Vec<f32>,
    master: Vec<f32>,
    start: u64,
    requested: u64,
}

async fn known_take(fragment: &[f32], level: f32) -> KnownTake {
    // WHY: The PCM reader plays one sample a frame on every channel; the
    // fixture interleaves two equal channels.
    let samples: Vec<f32> = fragment
        .iter()
        .step_by(usize::from(consts::CHANNELS))
        .map(|sample| sample * level)
        .collect();
    let frames = consts::KNOWN_BEATS * consts::KNOWN_PERIOD;
    let rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate");
    let session = HostConfig::offline(pools())
        .sample_rate(rate)
        .max_block_frames(NonZeroU32::new(consts::BLOCK_FRAMES).expect("test block size"))
        .metronome(
            MetronomeConfig::builder()
                .level(consts::TAKE_METRONOME)
                .duck(consts::TAKE_METRONOME)
                .build()
                .expect("take metronome"),
        )
        .build();
    let harness = play_resource(OfflinePlayer::new(session).await, move || {
        resource_from_reader(TestPcmReader::with_samples(
            AudioSpec::new(consts::CHANNELS, rate),
            samples,
        ))
    })
    .await;
    let host = harness.host();
    let tail =
        u64::try_from(consts::TAIL_BLOCKS).expect("tail blocks") * u64::from(consts::BLOCK_FRAMES);
    let mut master = host
        .attach_tap(Tap::Master, capacity(frames + 2 * tail))
        .await
        .expect("master tap");
    let mut output = host
        .attach_tap(Tap::Output, capacity(frames + 2 * tail))
        .await
        .expect("output tap");
    host.set_metronome(true).await.expect("metronome on");
    let start = host.position();
    let requested = host.position();
    let tempo = Tempo::new(f64::from(consts::KNOWN_BPM)).expect("fragment tempo");
    host.with(move |host| host.set_tempo(tempo))
        .await
        .expect("Host tempo");
    while host.position() < start + frames + tail {
        render_blocks(&harness, 1).await;
    }
    harness.close().await;

    assert_eq!(
        (output.drops(), master.drops()),
        (0, 0),
        "the taps keep every frame"
    );
    KnownTake {
        output: output.drain(),
        master: master.drain(),
        start,
        requested,
    }
}

#[kithara::test(tokio)]
async fn a_known_tempo_deck_and_the_metronome_keep_one_offset_on_every_beat(
    warp_nominal_clicks_long: Vec<f32>,
) {
    let full = known_take(&warp_nominal_clicks_long, consts::FRAGMENT_LEVEL).await;
    let half = known_take(&warp_nominal_clicks_long, consts::FRAGMENT_LEVEL / 2.0).await;

    if let Some(mut artifact) =
        AudioArtifactTap::from_env(&artifact_label(), consts::SAMPLE_RATE, consts::CHANNELS)
            .expect("listening artifact")
    {
        artifact.push(&full.output);
    }
    assert_eq!(
        (half.start, half.requested),
        (full.start, full.requested),
        "both takes run on one timeline"
    );
    assert!(
        half.master
            .iter()
            .copied()
            .eq(full.master.iter().map(|sample| sample / 2.0)),
        "the deck at half its level mixes to exactly half the master"
    );
    let onsets: Vec<u64> = clicks(&full.master)
        .iter()
        .map(|click| click.frame + full.start)
        .collect();
    let first_onset = *onsets.first().expect("the deck sounds the fragment");
    assert_eq!(
        onsets,
        (0..consts::KNOWN_BEATS)
            .map(|beat| first_onset + beat * consts::KNOWN_PERIOD)
            .collect::<Vec<_>>(),
        "the deck plays every fragment click one known period apart"
    );
    // WHY: The full take's output less the half take's cancels the click
    // and leaves the deck under the duck: what the listener hears of it.
    let deck: Vec<f32> = full
        .output
        .iter()
        .zip(&half.output)
        .map(|(full, half)| full - half)
        .collect();
    assert_eq!(
        clicks(&deck)
            .iter()
            .map(|click| click.frame + full.start)
            .collect::<Vec<_>>(),
        onsets,
        "the output sounds every fragment click on the frame the master mixes it"
    );

    // WHY: Each output frame is the frame's mix under the duck plus the
    // click, and both takes duck and click alike. Twice the half take's
    // output less the full take's cancels the mix and leaves the click.
    let clicked: Vec<f32> = half
        .output
        .iter()
        .zip(&full.output)
        .map(|(half, full)| half.mul_add(2.0, -full))
        .collect();
    let heard = clicks(&clicked);
    let end = full.start
        + u64::try_from(full.output.len()).expect("take length") / u64::from(consts::CHANNELS);
    let beats: Vec<(u64, bool)> = (0..)
        .map(|beat: u64| {
            (
                full.requested + beat * consts::KNOWN_PERIOD,
                beat % u64::from(consts::BEATS_PER_BAR) == 0,
            )
        })
        .take_while(|(frame, _)| frame + consts::SILENT_FOOT < end)
        .collect();
    // WHY: The deck steps by the known period from its first onset and the
    // metronome from the frame the tempo is set on, so the deck keeps one
    // offset from the metronome on every beat: no drift.
    assert_eq!(
        heard
            .iter()
            .map(|click| click.frame + full.start)
            .collect::<Vec<_>>(),
        beats
            .iter()
            .map(|(frame, _)| frame + consts::SILENT_FOOT)
            .collect::<Vec<_>>(),
        "the metronome clicks every computed beat of the known tempo from the frame it is set on"
    );
    assert_click_levels(&heard, &beats, consts::TAKE_METRONOME);
}
