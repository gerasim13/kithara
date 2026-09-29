//! The beat grid the fixture build analysed for each rhythm fixture, held
//! against the score the fixture was rendered from.

use std::num::NonZeroU32;

use kithara::{
    analysis::{AnalysisFile, AnalysisFingerprint, BeatGridModel},
    host::Metronome,
    signal::SessionFrame,
    warp::{SessionAnchor, SessionBeat},
};
use kithara_integration_tests::{
    audio_artifact::{AudioArtifactTap, artifact_label},
    grid::analysed_grid,
    kithara,
};
use kithara_test_fixtures::assets::by_name;
use num_traits::cast::ToPrimitive;

mod consts {
    pub(super) const STYLES: [&str; 6] = [
        "ambient_dub_62",
        "trip_hop_74",
        "downtempo_96",
        "house_124",
        "techno_132",
        "breakbeat_140",
    ];
    pub(super) const ANALYSIS_FINGERPRINT: &str = "rhythm-fixture:v1";
    pub(super) const SAMPLE_RATE: u32 = 48_000;
    pub(super) const CHANNELS: u16 = 2;
    pub(super) const WAV_HEADER_BYTES: usize = 44;
    /// How far a beat of the grid may sit from the score's and still be
    /// that beat of it: the fit's own residual.
    pub(super) const SAME_BEAT_SECONDS: f64 = 0.025;
    /// How much the grid's distance from the score may change from one beat
    /// to the next beyond the steady drift of its tempo. A grid read straight
    /// off 20 ms model frames jumps by up to a whole frame between beats; a
    /// line keeps one step, up to the score's rounding to whole frames.
    pub(super) const WOBBLE_SECONDS: f64 = 1e-4;
    /// How far the grid's beat period may be off the score's, as a share of
    /// it: a dozen seconds of markers a few milliseconds either side of the
    /// beat state the slope of the line through them to about a tenth of a
    /// percent.
    pub(super) const PERIOD_ERROR_RATIO: f64 = 2e-3;
    pub(super) const SECONDS_PER_MINUTE: f64 = 60.0;
    pub(super) const BEATS_PER_BAR: f64 = 4.0;
}

/// The beats of `style`'s score, in seconds.
fn score(style: &str) -> Vec<f64> {
    let name = format!("rhythm_expected_analysis_{style}_aligned");
    let asset = by_name(&name).unwrap_or_else(|| panic!("`{name}` is not registered"));
    let file = AnalysisFile::parse(
        asset.bytes(),
        &AnalysisFingerprint::new(Some(consts::ANALYSIS_FINGERPRINT), None),
    )
    .unwrap_or_else(|error| panic!("decode `{name}`: {error:?}"));
    let analysis = file.latest().analysis();
    let rate = f64::from(analysis.source_sample_rate().get());
    analysis
        .beat()
        .unwrap_or_else(|| panic!("`{name}` has no beats"))
        .artifact()
        .beats()
        .iter()
        .map(|frame| frame.to_f64().expect("a fixture frame") / rate)
        .collect()
}

/// How `grid` sits on one score: the grid beat each score beat falls on,
/// and how far after the score beat it sits.
#[derive(Debug)]
struct Fit {
    ordinals: Vec<i64>,
    offsets: Vec<f64>,
}

impl Fit {
    fn new(grid: &BeatGridModel, score: &[f64]) -> Self {
        let beats = &grid.as_raw().beats;
        let (ordinals, offsets) = score
            .iter()
            .map(|&at| {
                let nearest = beats
                    .iter()
                    .min_by(|a, b| (a.at - at).abs().total_cmp(&(b.at - at).abs()))
                    .expect("the grid states beats");
                (nearest.ordinal, nearest.at - at)
            })
            .unzip();
        Self { ordinals, offsets }
    }

    /// The most the grid's distance from the score changes from one beat to
    /// the next beyond the change the beat before made.
    fn wobble(&self) -> f64 {
        self.offsets
            .windows(3)
            .map(|step| step[1].mul_add(-2.0, step[2] + step[0]).abs())
            .fold(0.0, f64::max)
    }

    fn worst(&self) -> f64 {
        self.offsets
            .iter()
            .fold(0.0, |worst, offset| worst.max(offset.abs()))
    }
}

/// The fixture `track`'s audio as interleaved samples.
fn pcm(track: &str) -> Vec<f32> {
    let asset = by_name(track).unwrap_or_else(|| panic!("`{track}` is not registered"));
    asset.bytes()[consts::WAV_HEADER_BYTES..]
        .chunks_exact(2)
        .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0)
        .collect()
}

/// The engine metronome clicking on every beat of `grid` over `frames`
/// frames, loudest on its bar lines, interleaved like the track.
fn clicks(grid: &BeatGridModel, frames: usize) -> Vec<f32> {
    let raw = grid.as_raw();
    let first = raw.beats.first().expect("the grid states beats");
    let bar = raw.meter.map_or(0, |meter| meter.origin_beat_ordinal);
    let rate = NonZeroU32::new(consts::SAMPLE_RATE).expect("fixture rate");
    let anchor = SessionAnchor::new(
        SessionFrame::new(
            (first.at * f64::from(consts::SAMPLE_RATE))
                .round()
                .to_i64()
                .expect("a fixture frame"),
        ),
        SessionBeat::new(
            (first.ordinal - bar).to_f64().expect("an ordinal") + consts::BEATS_PER_BAR,
        )
        .expect("a finite beat"),
        raw.bpm / consts::SECONDS_PER_MINUTE,
        rate,
    )
    .expect("the grid's tempo is a session tempo");
    let mut mono = vec![0.0; frames];
    Metronome::default().render(Some(anchor), SessionFrame::new(0), &mut mono);
    mono.iter()
        .flat_map(|&sample| std::iter::repeat_n(sample, usize::from(consts::CHANNELS)))
        .collect()
}

/// A grid read off the detector's frames puts each beat up to half a frame
/// either side of the music; the grid a pass states is the line through
/// them, one beat period all the way along, near the score's.
///
/// A score beat in the track's last residual is left out: a grid a residual
/// from the score may place it past the end of the track.
#[kithara::test(native)]
fn the_analysed_grid_runs_parallel_to_the_score() {
    let mut failures = Vec::new();
    for style in consts::STYLES {
        let track = format!("rhythm_wav_{style}_aligned");
        let grid = analysed_grid(&track);
        let length = grid.as_raw().duration.expect("a fixture states its length");
        let score: Vec<f64> = score(style)
            .into_iter()
            .filter(|at| at + consts::SAME_BEAT_SECONDS <= length)
            .collect();
        let fit = Fit::new(&grid, &score);

        if let Some(mut artifact) = AudioArtifactTap::from_env(
            &format!("{}_{style}", artifact_label()),
            consts::SAMPLE_RATE,
            consts::CHANNELS,
        )
        .expect("listening artifact")
        {
            let pcm = pcm(&track);
            artifact.push(&pcm);
            artifact.push_metronome(&clicks(&grid, pcm.len() / usize::from(consts::CHANNELS)));
        }

        let score_period =
            (score[score.len() - 1] - score[0]) / (score.len() - 1).to_f64().expect("a beat count");
        let period = consts::SECONDS_PER_MINUTE / grid.as_raw().bpm;
        let period_error = (period - score_period).abs() / score_period;
        let consecutive = fit.ordinals.windows(2).all(|pair| pair[1] == pair[0] + 1);
        if !consecutive
            || fit.worst() > consts::SAME_BEAT_SECONDS
            || fit.wobble() > consts::WOBBLE_SECONDS
            || period_error > consts::PERIOD_ERROR_RATIO
        {
            failures.push(format!(
                "{style}: period {period:.6} s vs score {score_period:.6} s ({period_error:.2e}), \
                 wobble {:.3} ms, worst {:.2} ms, ordinals {:?}, offsets (ms) {:?}",
                fit.wobble() * 1e3,
                fit.worst() * 1e3,
                fit.ordinals,
                fit.offsets
                    .iter()
                    .map(|offset| (offset * 1e4).round() / 10.0)
                    .collect::<Vec<_>>(),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the analysed grid does not keep one tempo near the score's:\n{}",
        failures.join("\n")
    );
}
