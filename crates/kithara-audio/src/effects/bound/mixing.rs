//! Two records at different tempos, placed on one session grid, must strike
//! their beats together. That is what a mix *is*, and it is the property the
//! rest of this module exists to serve — every other oracle here measures a
//! span or a count, none of them measure the sound.

use std::{f64::consts::TAU, num::NonZeroU32};

use kithara_bufpool::PcmPool;
use kithara_decode::{PcmChunk, PcmMeta, PcmSpec};
use kithara_events::PlaybackDirection;
use kithara_platform::sync::Arc;
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;

use super::bound_slot;
use crate::{
    analysis::TrackAnalysis,
    musical::{
        SessionAnchor, SessionAnchorCell, SessionBeat, SessionFrame, SourceSchedule, TrackBeat,
        TrackBeatMap,
    },
    traits::AudioEffect,
    waveform::BeatGrid,
};

struct Consts;

impl Consts {
    const RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;
    /// A slow record.
    const SLOW_BPM: f64 = 96.0;
    /// A fast one. Neither matches the session, so both must stretch, in
    /// opposite directions.
    const FAST_BPM: f64 = 128.0;
    /// What the mix runs at.
    const SESSION_BPM: f64 = 120.0;
    /// Marker tones an octave apart, so the two decks are told apart by ear.
    const SLOW_HZ: f64 = 220.0;
    const FAST_HZ: f64 = 440.0;
    const SOURCE_SECS: f64 = 60.0;
    /// Beats of mix measured. Long enough that per-block rounding drift would
    /// accumulate past a frame if any existed.
    const MEASURED_BEATS: usize = 64;
    const CHUNK_FRAMES: usize = 512;
    /// Silence floor an onset must cross.
    const ONSET_FLOOR: f32 = 0.02;
    /// Burst length as a fraction of a beat: short enough never to overlap.
    const BURST_OF_BEAT: f64 = 0.12;
}

/// The timbre a deck marks its beats with. Two decks summed are
/// indistinguishable by amplitude alone; a sine carries only its fundamental
/// and a square the odd harmonics above it, so the mix can be told apart by
/// ear and by spectrum.
#[derive(Clone, Copy)]
enum Pulse {
    Sine,
    Square,
}

impl Pulse {
    fn at(self, phase: f64) -> f64 {
        match self {
            Self::Sine => phase.sin(),
            Self::Square => {
                if phase.sin() >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

fn rate() -> NonZeroU32 {
    NonZeroU32::new(Consts::RATE).expect("invariant: fixture rate is non-zero")
}

fn spec() -> PcmSpec {
    PcmSpec::new(Consts::CHANNELS, rate())
}

/// A metronome as source audio: silence with one short burst on every beat of
/// its own tempo. The burst attacks on the first frame of the beat, so an
/// onset in the rendered output *is* where that beat landed — no analysis step
/// stands between the measurement and the claim.
fn pulse_track(bpm: f64, shape: Pulse, tone_hz: f64) -> (Vec<f32>, Vec<u64>) {
    let exact = |value: f64| {
        value
            .round()
            .to_u64()
            .expect("invariant: fixture magnitudes are small and positive")
    };
    let rate = f64::from(Consts::RATE);
    let frames = exact(rate * Consts::SOURCE_SECS);
    let beat_frames = exact(rate * 60.0 / bpm);
    let burst = exact(
        beat_frames
            .to_f64()
            .expect("invariant: fixture frame counts are exact in f64")
            * Consts::BURST_OF_BEAT,
    );
    let channels = usize::from(Consts::CHANNELS);
    let len = usize::try_from(frames).expect("invariant: the fixture fits memory") * channels;
    let mut pcm = vec![0.0_f32; len];
    for frame in 0..frames {
        let into_beat = frame % beat_frames;
        if into_beat >= burst {
            continue;
        }
        let decay = 1.0
            - into_beat
                .to_f64()
                .expect("invariant: fixture frame counts are exact in f64")
                / burst
                    .to_f64()
                    .expect("invariant: fixture frame counts are exact in f64");
        let phase = TAU
            * tone_hz
            * into_beat
                .to_f64()
                .expect("invariant: fixture frame counts are exact in f64")
            / rate;
        let value = shape.at(phase) * 0.6 * decay * decay;
        let at = usize::try_from(frame).expect("invariant: the fixture fits memory") * channels;
        for channel in 0..channels {
            pcm[at + channel] = value as f32;
        }
    }
    let markers = (0..frames.div_ceil(beat_frames))
        .map(|beat| beat * beat_frames)
        .collect();
    (pcm, markers)
}

/// The one grid both decks follow.
fn session_grid() -> Arc<SessionAnchorCell> {
    let cell = SessionAnchorCell::new();
    cell.publish(
        SessionAnchor::new(
            SessionFrame::new(0),
            SessionBeat::default(),
            Consts::SESSION_BPM / 60.0,
            rate(),
        )
        .expect("invariant: the fixture tempo is a positive rate"),
    );
    cell
}

/// Renders one deck through the bound slot: its own tempo in, the session's
/// grid out.
fn render_deck(
    bpm: f64,
    shape: Pulse,
    tone_hz: f64,
    grid: &Arc<SessionAnchorCell>,
    frames: usize,
) -> Vec<f32> {
    let (source, markers) = pulse_track(bpm, shape, tone_hz);
    let source_frames = u64::try_from(source.len() / usize::from(Consts::CHANNELS))
        .expect("invariant: the fixture length fits u64");
    let analysis = TrackAnalysis::with_source_rate(
        Some(BeatGrid::new(bpm, markers, vec![0], Vec::new())),
        None,
        source_frames,
        rate(),
    );
    let map = TrackBeatMap::new(&analysis, rate()).expect("invariant: fixture markers form a map");
    let schedule = Arc::new(SourceSchedule::new(
        map,
        TrackBeat::default(),
        PlaybackDirection::Forward,
        Arc::clone(grid),
    ));
    let mut slot = bound_slot(schedule, spec(), PcmPool::default())
        .expect("invariant: an exact-span engine is compiled in");

    let channels = usize::from(Consts::CHANNELS);
    let want = frames * channels;
    let mut out: Vec<f32> = Vec::with_capacity(want);
    let mut fed = 0_usize;
    let available = usize::try_from(source_frames).expect("invariant: the fixture fits memory");
    while out.len() < want && fed + Consts::CHUNK_FRAMES <= available {
        let slice = &source[fed * channels..(fed + Consts::CHUNK_FRAMES) * channels];
        let chunk = PcmChunk::new(
            PcmMeta {
                spec: spec(),
                frames: u32::try_from(Consts::CHUNK_FRAMES).expect("invariant: the chunk fits u32"),
                frame_offset: u64::try_from(fed).expect("invariant: the offset fits u64"),
                ..Default::default()
            },
            PcmPool::default().attach(slice.to_vec()),
        );
        fed += Consts::CHUNK_FRAMES;
        if let Some(rendered) = slot.process(chunk) {
            out.extend_from_slice(&rendered.samples);
        }
    }
    out.truncate(want);
    out
}

/// Frames where a burst begins.
///
/// A refractory window is not a tolerance — it is what makes the measurement
/// mean "beat" at all. A square marker crosses the silence floor on every
/// cycle of its own tone, so a bare threshold counts oscillations, not
/// attacks. Nothing shorter than half a beat can be a second beat.
fn onsets(pcm: &[f32]) -> Vec<usize> {
    let channels = usize::from(Consts::CHANNELS);
    let refractory = session_beat_frames() / 2;
    let mut found: Vec<usize> = Vec::new();
    for (frame, samples) in pcm.chunks_exact(channels).enumerate() {
        let peak = samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        if peak <= Consts::ONSET_FLOOR {
            continue;
        }
        if found.last().is_none_or(|last| frame - last >= refractory) {
            found.push(frame);
        }
    }
    found
}

/// Output frames between session beats at the mix tempo.
fn session_beat_frames() -> usize {
    (f64::from(Consts::RATE) * 60.0 / Consts::SESSION_BPM)
        .round()
        .to_usize()
        .expect("invariant: one fixture beat fits usize")
}

fn decks(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let grid = session_grid();
    (
        render_deck(
            Consts::SLOW_BPM,
            Pulse::Sine,
            Consts::SLOW_HZ,
            &grid,
            frames,
        ),
        render_deck(
            Consts::FAST_BPM,
            Pulse::Square,
            Consts::FAST_HZ,
            &grid,
            frames,
        ),
    )
}

/// Neither deck drifts from the session grid.
///
/// Distance is the discriminator, not a tolerance. A per-block rounding error
/// multiplies with the number of beats; a wobble in where a threshold crosses
/// a stretched attack does not. So the same span error is measured over a
/// quarter of the run and over all of it: drift would grow four-fold, jitter
/// stays put.
#[kithara::test]
fn neither_deck_drifts_from_the_session_grid() {
    let per_beat = session_beat_frames();
    let (slow, fast) = decks(per_beat * Consts::MEASURED_BEATS);

    for (name, rendered) in [("96 BPM", slow), ("128 BPM", fast)] {
        let beats = onsets(&rendered);
        assert!(beats.len() > 16, "{name}: the deck must strike repeatedly");
        let near = span_error(&beats, beats.len() / 4, per_beat);
        let far = span_error(&beats, beats.len() - 1, per_beat);

        assert!(
            far <= near.max(4) * 2,
            "{name}: the error grew with distance — {near} frames over {} beats, \
             {far} over {}; that is drift, not measurement",
            beats.len() / 4,
            beats.len() - 1
        );
    }
}

/// Frames between the first and `beats`-th onset, against what the session
/// grid puts there. The opening onset is skipped: a stretched attack crosses
/// the floor a few frames off its own edge, and that belongs to the
/// measurement rather than the grid.
fn span_error(beats: &[usize], count: usize, per_beat: usize) -> usize {
    let measured = &beats[1..=count];
    let span = measured[measured.len() - 1] - measured[0];
    span.abs_diff((measured.len() - 1) * per_beat)
}

/// The two decks do not drift apart. Same discriminator: a separation that
/// opens up grows with distance, one that merely wobbles does not.
#[kithara::test]
fn the_decks_do_not_drift_apart() {
    let per_beat = session_beat_frames();
    let (slow, fast) = decks(per_beat * Consts::MEASURED_BEATS);

    let slow_beats = onsets(&slow);
    let fast_beats = onsets(&fast);
    let pairs = slow_beats.len().min(fast_beats.len());
    assert!(pairs > 16, "both decks must strike repeatedly");

    let separation = |at: usize| fast_beats[at].abs_diff(slow_beats[at]) as i64;
    let opened_near = (separation(pairs / 4) - separation(1)).abs();
    let opened_far = (separation(pairs - 1) - separation(1)).abs();

    assert!(
        opened_far <= opened_near.max(4) * 2,
        "the decks drifted apart: separation opened by {opened_near} frames over {} beats \
         and {opened_far} over {}",
        pairs / 4,
        pairs - 1
    );
}

/// The separation they *do* hold is the engine's content delay, and it is not
/// the same for both decks.
///
/// An exact-span engine carries its algorithmic latency as delayed content,
/// declared in **source** frames. Two decks stretching by different ratios
/// therefore emerge offset from each other by a constant the slot never
/// compensates. It is not drift and it does not grow, but it is a flam, and a
/// mix cannot be called aligned while it stands. Pinned here at its measured
/// size so the compensation that removes it has something to move.
#[kithara::test]
fn the_uncompensated_engine_delay_offsets_the_decks() {
    let per_beat = session_beat_frames();
    let (slow, fast) = decks(per_beat * Consts::MEASURED_BEATS);

    let separation = onsets(&fast)[1].abs_diff(onsets(&slow)[1]);

    assert!(
        separation > 0,
        "the delay is uncompensated today; a zero here means it was fixed and \
         this oracle should become the alignment one"
    );
    assert!(
        separation < per_beat / 8,
        "the offset is a flam, not a missed beat: {separation} frames"
    );
}

/// The unbound reference: left alone, a 96 BPM record keeps its own spacing
/// and never matches the session's. Without this the oracles above would hold
/// just as well against a slot that does nothing.
#[kithara::test]
fn an_unstretched_record_keeps_its_own_spacing() {
    let per_beat = session_beat_frames();
    let (source, _) = pulse_track(Consts::SLOW_BPM, Pulse::Sine, Consts::SLOW_HZ);

    let beats = onsets(&source);
    let spacing = beats[1] - beats[0];

    assert!(
        spacing.abs_diff(per_beat) > 1,
        "an unstretched 96 BPM record must not already be spaced like 120 BPM"
    );
}

/// The mix carries both decks rather than cancelling them.
#[kithara::test]
fn the_mix_carries_both_decks() {
    let (slow, fast) = decks(session_beat_frames() * 4);
    let summed: Vec<f32> = slow.iter().zip(&fast).map(|(a, b)| (a + b) * 0.5).collect();
    let peak = |pcm: &[f32]| pcm.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));

    assert!(peak(&summed) > peak(&slow).max(peak(&fast)) * 0.4);
}

/// Writes the mix to disk so it can be judged by ear, which is the only judge
/// that matters for a mix.
///
/// Three files: each deck alone and the sum. The sine and the square are an
/// octave apart, so in the sum they are separable by ear — a flam between them
/// is heard as two strikes, and alignment as one.
#[kithara::test]
#[ignore = "diagnostic: writes /tmp/kithara_mix/*.wav for offline listening; run with --run-ignored"]
fn dump_the_mix_for_listening() {
    let dir = std::path::PathBuf::from("/tmp/kithara_mix");
    std::fs::create_dir_all(&dir).expect("invariant: the dump directory is writable");

    let frames = session_beat_frames() * Consts::MEASURED_BEATS;
    let (slow, fast) = decks(frames);
    let summed: Vec<f32> = slow.iter().zip(&fast).map(|(a, b)| (a + b) * 0.5).collect();

    for (name, pcm) in [
        ("01_deck_a_96bpm_sine.wav", &slow),
        ("02_deck_b_128bpm_square.wav", &fast),
        ("03_mix_on_a_120bpm_grid.wav", &summed),
    ] {
        write_wav(&dir.join(name), pcm);
    }
}

/// Interleaved 16-bit PCM, the shape every player opens without asking.
#[cfg(test)]
fn write_wav(path: &std::path::Path, pcm: &[f32]) {
    use std::io::Write as _;

    let channels = u16::from(Consts::CHANNELS);
    let bits = 16_u16;
    let block_align = channels * bits / 8;
    let byte_rate = Consts::RATE * u32::from(block_align);
    let data_len = u32::try_from(pcm.len() * 2).expect("invariant: the dump fits a wav");

    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&Consts::RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        let clipped = sample.clamp(-1.0, 1.0) * f32::from(i16::MAX);
        out.extend_from_slice(&(clipped as i16).to_le_bytes());
    }

    std::fs::File::create(path)
        .and_then(|mut file| file.write_all(&out))
        .expect("invariant: the dump is writable");
}
