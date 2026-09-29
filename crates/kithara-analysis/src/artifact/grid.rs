use std::{collections::BTreeMap, num::NonZeroU16};

use kithara_beat::{
    BeatGridError, BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid,
    SCHEMA_VERSION,
};
use num_traits::cast::ToPrimitive;
use thiserror::Error;

use super::{
    meter::voted_bar,
    snapshot::BeatState,
    steady::{Line, SteadyRun},
    track::TrackAnalysis,
};
use crate::consts;

/// Why a pass states no grid a player could follow.
///
/// The pass keeps its own artifact either way: a grid it cannot state is not a
/// waveform it cannot publish.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum BeatGridUnavailable {
    #[error("the pass published no beat artifact")]
    NoBeats,
    #[error(transparent)]
    Rejected(#[from] BeatGridError),
    #[error("the artifact carries {bpm} where a tempo would be")]
    Tempo { bpm: f64 },
    #[error("the pass's beat markers do not strictly rise")]
    Unordered,
    #[error("no run of the pass's beat markers keeps one tempo long enough to follow")]
    NoSteadyRun,
}

/// Restates one published pass as the server-side grid contract.
///
/// The grid is the line through the longest steady run of observed markers,
/// stated as every whole beat of that line from the start of the track to the
/// end of what is known of it: a marker only ever lands near a beat, and the
/// line through many of them is where the beats are. A beat a marker named
/// carries that marker's confidence; one the line alone places carries none.
/// Beat 0 is the beat of the line nearest the start of the track, so a
/// revision that moves the line by a hair keeps every beat's number unless
/// the line's beats stand half a beat either side of the start, where any
/// count from the start turns over; a reader following beats across
/// revisions knows a beat by where it stands.
///
/// This is the only place source frames become media seconds: the artifact
/// keeps frames, and the model has no sample rate to read them against.
impl TryFrom<&TrackAnalysis> for BeatGridModel {
    type Error = BeatGridUnavailable;

    fn try_from(analysis: &TrackAnalysis) -> Result<Self, Self::Error> {
        let snapshot = analysis.beat().ok_or(BeatGridUnavailable::NoBeats)?;
        let artifact = snapshot.artifact();
        let stated = artifact.bpm();
        if !stated.is_finite() || stated <= 0.0 {
            return Err(BeatGridUnavailable::Tempo { bpm: stated });
        }
        let rate = f64::from(analysis.source_sample_rate().get());
        let horizon = seconds(analysis.source_frames(), rate);
        let fit = analysis.grid_fit();
        let (times, confidences) =
            observed(artifact.beats(), artifact.beat_confidence(), rate, horizon);
        if times.windows(2).any(|pair| pair[1] <= pair[0]) {
            return Err(BeatGridUnavailable::Unordered);
        }
        let run = fit
            .steady_run(&times, consts::SECONDS_PER_MINUTE / stated)
            .ok_or(BeatGridUnavailable::NoSteadyRun)?;
        let beats = beats(&run, &confidences, horizon);
        let (heard, heard_confidences) = observed(
            artifact.downbeats(),
            artifact.downbeat_confidence(),
            rate,
            horizon,
        );
        let (downbeats, meter) = bars(
            &beats,
            run.line,
            heard.iter().copied().zip(heard_confidences),
            fit.residual.as_secs_f64(),
        );
        Ok(Self::try_from(RawBeatGrid {
            duration: analysis.extent().map(|extent| seconds(extent, rate)),
            bpm: consts::SECONDS_PER_MINUTE / run.line.period,
            downbeats,
            meter,
            schema_version: SCHEMA_VERSION,
            model_id: analysis.token().as_str().to_owned(),
            revision: analysis.revision(),
            state: match snapshot.state() {
                BeatState::Final => BeatGridState::Final,
                BeatState::Provisional => BeatGridState::Provisional,
            },
            beats,
        })?)
    }
}

/// The markers a detector saw, in media seconds up to `horizon`, beside the
/// confidence it saw each with. A marker analysis placed by extrapolation is
/// not an observation, and the line is fitted to observations only.
fn observed(
    frames: &[u64],
    confidences: &[Option<f32>],
    rate: f64,
    horizon: f64,
) -> (Vec<f64>, Vec<f32>) {
    frames
        .iter()
        .zip(confidences)
        .filter_map(|(frame, confidence)| Some((seconds(*frame, rate), (*confidence)?)))
        .filter(|(at, _)| *at <= horizon)
        .unzip()
}

/// Every whole beat of the run's line from the start of the track to
/// `horizon`.
fn beats(run: &SteadyRun, confidences: &[f32], horizon: f64) -> Vec<GridBeat> {
    let line = run.line;
    let Some(last) = line.nearest(horizon) else {
        return Vec::new();
    };
    let heard: BTreeMap<i64, f32> = run
        .members
        .iter()
        .map(|&(ordinal, index)| (ordinal, confidences[index]))
        .collect();
    (0..=last)
        .map(|ordinal| GridBeat {
            at: line.at(ordinal),
            ordinal,
            confidence: heard.get(&ordinal).copied(),
        })
        .filter(|beat| (0.0..=horizon).contains(&beat.at))
        .collect()
}

/// The bar lines the detected ones agree on, each on the beat of the grid it
/// falls on, and the meter they keep.
///
/// Only a detected bar line within `residual` of a beat of the line votes;
/// the bar and phase the votes agree on then name every beat of that phase a
/// bar line, so a bar the detector skipped is stated without a confidence of
/// its own and a bar line on the wrong beat is left out. Votes that agree on
/// nothing state no bars at all.
fn bars(
    beats: &[GridBeat],
    line: Line,
    heard: impl Iterator<Item = (f64, f32)>,
    residual: f64,
) -> (Vec<GridDownbeat>, Option<Meter>) {
    let mut votes: BTreeMap<i64, f32> = BTreeMap::new();
    for (at, confidence) in heard {
        if let Some(ordinal) = line.beat_of(at, residual) {
            votes.entry(ordinal).or_insert(confidence);
        }
    }
    let Some((bar, phase)) = voted_bar(votes.keys().copied()) else {
        return (Vec::new(), None);
    };
    let downbeats: Vec<GridDownbeat> = beats
        .iter()
        .filter(|beat| beat.ordinal.rem_euclid(bar) == phase)
        .map(|beat| GridDownbeat {
            at: beat.at,
            beat_ordinal: beat.ordinal,
            confidence: votes.get(&beat.ordinal).copied(),
        })
        .collect();
    let meter = bar
        .to_u16()
        .and_then(NonZeroU16::new)
        .zip(downbeats.first())
        .map(|(beats_per_bar, first)| Meter {
            beats_per_bar,
            origin_beat_ordinal: first.beat_ordinal,
        });
    (downbeats, meter)
}

/// A frame the timeline cannot represent becomes a position the contract
/// refuses, rather than one it silently rounds.
fn seconds(frame: u64, rate: f64) -> f64 {
    frame.to_f64().unwrap_or(f64::INFINITY) / rate
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_beat::{BeatGridModel, BeatGridState};
    use kithara_test_utils::kithara;
    use num_traits::cast::ToPrimitive;
    use rangemap::RangeSet;

    use super::{BeatGridUnavailable, BeatState, TrackAnalysis};
    use crate::{BeatArtifact, BeatSnapshot, artifact::track::AnalysisToken, consts};

    /// A pass over `beats` and `downbeats`, observed with full confidence,
    /// that has read the source up to `extent` or, when the length is not
    /// known yet, to just past its last beat.
    fn analysis(
        rate: u32,
        beats: &[u64],
        downbeats: &[u64],
        extent: Option<u64>,
        state: BeatState,
    ) -> TrackAnalysis {
        artifact_analysis(
            rate,
            BeatArtifact::new(
                consts::BPM,
                beats.iter().map(|frame| (*frame, Some(1.0))).collect(),
                downbeats.iter().map(|frame| (*frame, Some(0.9))).collect(),
            ),
            extent,
            state,
        )
    }

    fn artifact_analysis(
        rate: u32,
        artifact: BeatArtifact,
        extent: Option<u64>,
        state: BeatState,
    ) -> TrackAnalysis {
        let read = extent.unwrap_or_else(|| artifact.beats().last().map_or(0, |last| last + 1));
        let mut coverage = RangeSet::new();
        if read > 0 {
            coverage.insert(0..read);
        }
        TrackAnalysis::builder()
            .token(AnalysisToken::from("track-42"))
            .source_sample_rate(NonZeroU32::new(rate).expect("invariant: a fixture rate is set"))
            .beat(BeatSnapshot::new(artifact, state, Vec::new()))
            .maybe_extent(extent)
            .coverage(coverage)
            .revision(7)
            .build()
    }

    fn grid(analysis: &TrackAnalysis) -> BeatGridModel {
        BeatGridModel::try_from(analysis).expect("the pass states a grid")
    }

    /// Every beat of the grid as its ordinal and its time in whole
    /// microseconds, so a line fitted through exact markers compares exactly.
    fn times(model: &BeatGridModel) -> Vec<(i64, i64)> {
        model
            .as_raw()
            .beats
            .iter()
            .map(|beat| (beat.ordinal, micros(beat.at)))
            .collect()
    }

    fn micros(seconds: f64) -> i64 {
        (seconds * 1e6)
            .round()
            .to_i64()
            .expect("a fixture time fits in microseconds")
    }

    /// The ordinals of the beats a detected marker named.
    fn heard(model: &BeatGridModel) -> Vec<i64> {
        model
            .as_raw()
            .beats
            .iter()
            .filter(|beat| beat.confidence.is_some())
            .map(|beat| beat.ordinal)
            .collect()
    }

    fn ordinals(model: &BeatGridModel) -> Vec<i64> {
        model
            .as_raw()
            .beats
            .iter()
            .map(|beat| beat.ordinal)
            .collect()
    }

    fn on_beats(beats: impl Iterator<Item = u64>) -> Vec<u64> {
        beats.map(|beat| beat * consts::PERIOD_48).collect()
    }

    fn half_beats(half_beats: impl Iterator<Item = u64>) -> Vec<u64> {
        half_beats
            .map(|half| half * consts::PERIOD_48 / 2)
            .collect()
    }

    /// Detected frames become media seconds here and nowhere else, and the
    /// grid runs to the end of the media even where no marker was heard.
    #[kithara::test(native, flash(false))]
    fn a_pass_publishes_its_beats_as_media_seconds_on_its_own_grid() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..8),
            &[],
            Some(9 * consts::PERIOD_48),
            BeatState::Provisional,
        ));

        assert_eq!(
            model.as_raw().model_id,
            "track-42",
            "the token names the model"
        );
        assert_eq!(model.as_raw().revision, 7);
        assert_eq!(model.as_raw().state, BeatGridState::Provisional);
        assert_eq!(micros(model.as_raw().bpm), micros(consts::BPM));
        assert_eq!(
            model.as_raw().duration,
            Some(4.5),
            "the extent states the length"
        );
        assert_eq!(
            times(&model),
            (0..=9)
                .map(|beat| (beat, beat * 500_000))
                .collect::<Vec<_>>()
        );
        assert_eq!(heard(&model), (0..8).collect::<Vec<_>>());
    }

    /// Markers read off a detector's frames land up to half a frame either
    /// side of the beat; the grid is the beat itself, one line through them.
    #[kithara::test(native)]
    fn markers_jittered_around_the_beat_state_the_beat_itself() {
        let period = 0.483_87;
        let origin = 0.137;
        let frame = 0.02;
        let rate = f64::from(consts::RATE_48);
        let beats: Vec<(u64, Option<f32>)> = (0..40)
            .map(|beat| {
                let exact = f64::from(beat).mul_add(period, origin);
                let heard = ((exact / frame).round() * frame * rate)
                    .round()
                    .to_u64()
                    .expect("a fixture frame");
                (heard, Some(1.0))
            })
            .collect();
        let model = grid(&artifact_analysis(
            consts::RATE_48,
            BeatArtifact::new(consts::SECONDS_PER_MINUTE / period, beats, Vec::new()),
            Some(20 * u64::from(consts::RATE_48)),
            BeatState::Final,
        ));

        let worst = model
            .as_raw()
            .beats
            .iter()
            .map(|beat| {
                let ordinal = beat.ordinal.to_f64().expect("a small ordinal");
                (beat.at - ordinal.mul_add(period, origin)).abs()
            })
            .fold(0.0, f64::max);
        assert_eq!(heard(&model), (0..40).collect::<Vec<_>>());
        assert!(
            worst < 0.002,
            "every beat sits on the music, not on its marker: {worst} s off"
        );
    }

    /// The model carries no sample rate, so two passes over the same music
    /// must agree once their frames are read against their own rates.
    #[kithara::test(native, flash(false))]
    fn the_same_music_states_the_same_grid_from_either_source_rate() {
        let at_48 = on_beats(0..8);
        let at_44_1: Vec<u64> = (0..8).map(|beat| beat * consts::PERIOD_44_1).collect();

        assert_eq!(
            times(&grid(&analysis(
                consts::RATE_48,
                &at_48,
                &[],
                None,
                BeatState::Final
            ))),
            times(&grid(&analysis(
                consts::RATE_44_1,
                &at_44_1,
                &[],
                None,
                BeatState::Final
            )))
        );
    }

    /// The gap between two analysed islands costs the grid no beats, and the
    /// beats in it are the line's, claiming nothing was heard there.
    #[kithara::test(native, flash(false))]
    fn islands_keep_the_ordinals_the_music_gives_them() {
        let beats = on_beats((0..8).chain(60..68));
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(ordinals(&model), (0..68).collect::<Vec<_>>());
        assert_eq!(
            heard(&model),
            (0..8).chain(60..68).collect::<Vec<_>>(),
            "the ordinal counts beats of the music, never entries of the list"
        );
    }

    /// Music that starts after a silence still has beats from the start of
    /// the track, on the line its music keeps.
    #[kithara::test(native)]
    fn the_grid_reaches_back_over_an_intro_no_marker_was_heard_in() {
        let quarter = consts::PERIOD_48 / 4;
        let beats: Vec<u64> = on_beats(6..20)
            .into_iter()
            .map(|frame| frame + quarter)
            .collect();
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Final,
        ));

        assert_eq!(ordinals(&model), (0..20).collect::<Vec<_>>());
        assert_eq!(heard(&model), (6..20).collect::<Vec<_>>());
    }

    /// A track a hair slower than its stated tempo drifts a whole beat off
    /// the stated period within a few minutes; the grid follows the music's
    /// own tempo, and every marker stays one of its beats.
    #[kithara::test(native, flash(false))]
    fn a_tempo_drifting_from_the_stated_one_keeps_every_beat() {
        let slow = consts::PERIOD_48 + consts::PERIOD_48 / 100;
        let beats: Vec<u64> = (0..100)
            .map(|beat| beat * slow + consts::PERIOD_48 / 4)
            .collect();
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Final,
        ));

        assert_eq!(heard(&model), (0..100).collect::<Vec<_>>());
        assert_eq!(micros(model.as_raw().bpm), micros(consts::BPM / 1.01));
    }

    /// A tracker that slips half a beat onto the off-beats for a phrase and
    /// back names no beat there; the music on either side is one line of
    /// beats, counted across the slip.
    #[kithara::test(native, flash(false))]
    fn a_phrase_tracked_half_a_beat_off_leaves_the_beats_around_it_counted() {
        let beats = half_beats(
            (0..20)
                .map(|beat| 2 * beat)
                .chain((20..25).map(|beat| 2 * beat + 1))
                .chain((26..46).map(|beat| 2 * beat)),
        );
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Final,
        ));

        assert_eq!(ordinals(&model), (0..46).collect::<Vec<_>>());
        assert_eq!(heard(&model), (0..20).chain(26..46).collect::<Vec<_>>());
    }

    /// A stray first marker half a beat before the music is not a beat of
    /// it, and does not cost the music its beats.
    #[kithara::test(native, flash(false))]
    fn a_stray_first_marker_does_not_cost_the_music_its_beats() {
        let beats = half_beats(std::iter::once(0).chain((1..20).map(|beat| 2 * beat + 1)));
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Final,
        ));

        assert_eq!(
            model.as_raw().beats.first().map(|beat| micros(beat.at)),
            Some(250_000),
            "the grid keeps the music's phase, not the stray marker's"
        );
        assert_eq!(heard(&model).len(), 19);
    }

    /// A marker off the line names no beat, and the beat the line places
    /// there claims nothing was heard.
    #[kithara::test(native)]
    fn a_marker_off_the_line_is_not_a_beat() {
        let mut beats = on_beats(0..12);
        beats[2] = 40_000;
        let model = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(micros(model.as_raw().bpm), micros(consts::BPM));
        assert_eq!(ordinals(&model), (0..12).collect::<Vec<_>>());
        assert_eq!(
            heard(&model),
            [0, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            "a marker the line does not hold is not an observation of a beat"
        );
    }

    /// Too few markers keep one tempo to follow, so the pass states no grid
    /// rather than a line its markers do not hold.
    #[kithara::test(native)]
    fn markers_that_keep_no_tempo_state_no_grid() {
        let wandering: Vec<u64> = (0..24_u64)
            .scan(0, |at, step| {
                *at += consts::PERIOD_48 + (step % 5) * consts::PERIOD_48 / 7;
                Some(*at)
            })
            .collect();

        assert_eq!(
            BeatGridModel::try_from(&analysis(
                consts::RATE_48,
                &wandering,
                &[],
                None,
                BeatState::Final,
            )),
            Err(BeatGridUnavailable::NoSteadyRun)
        );
    }

    /// Markers that do not strictly rise are no pass's observations: the fit
    /// is never asked to find a tempo in them.
    #[kithara::test(native)]
    fn markers_that_do_not_rise_state_no_grid() {
        let mut repeated = on_beats(0..12);
        repeated[6] = repeated[5];

        assert_eq!(
            BeatGridModel::try_from(&analysis(
                consts::RATE_48,
                &repeated,
                &[],
                None,
                BeatState::Final,
            )),
            Err(BeatGridUnavailable::Unordered)
        );
    }

    /// A pass that does not yet know the length still states what it heard,
    /// up to where it has read.
    #[kithara::test(native, flash(false))]
    fn an_unknown_length_does_not_withhold_the_grid() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..8),
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(model.as_raw().duration, None);
        assert_eq!(ordinals(&model), (0..8).collect::<Vec<_>>());
    }

    #[kithara::test(native, flash(false))]
    fn a_later_pass_publishes_the_same_grid_as_final() {
        let beats = on_beats(0..8);
        let provisional = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            Some(8 * consts::PERIOD_48),
            BeatState::Provisional,
        ));
        let final_pass = grid(&analysis(
            consts::RATE_48,
            &beats,
            &[],
            Some(8 * consts::PERIOD_48),
            BeatState::Final,
        ));

        assert_eq!(final_pass.as_raw().state, BeatGridState::Final);
        assert_eq!(
            times(&final_pass),
            times(&provisional),
            "settling changes the claim, not the beats"
        );
        assert_eq!(final_pass.as_raw().model_id, provisional.as_raw().model_id);
    }

    /// An artifact built entirely by extrapolation reports no tempo, and a
    /// typed refusal is what a caller gets - the waveform is untouched.
    #[kithara::test(native, flash(false))]
    fn a_degraded_artifact_states_no_grid() {
        let degraded = artifact_analysis(
            consts::RATE_48,
            BeatArtifact::new(0.0, vec![(0, None), (100, None)], Vec::new()),
            Some(consts::PERIOD_48),
            BeatState::Final,
        );

        assert_eq!(
            BeatGridModel::try_from(&degraded),
            Err(BeatGridUnavailable::Tempo { bpm: 0.0 })
        );
    }

    /// Beats analysis placed by extrapolation are not observations, and a
    /// line is never fitted through them alone.
    #[kithara::test(native)]
    fn extrapolated_beats_state_no_grid() {
        let extrapolated = artifact_analysis(
            consts::RATE_48,
            BeatArtifact::new(
                consts::BPM,
                on_beats(0..16)
                    .into_iter()
                    .map(|frame| (frame, None))
                    .collect(),
                Vec::new(),
            ),
            None,
            BeatState::Final,
        );

        assert_eq!(
            BeatGridModel::try_from(&extrapolated),
            Err(BeatGridUnavailable::NoSteadyRun)
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_pass_without_a_beat_artifact_states_no_grid() {
        let waveform_only = TrackAnalysis::builder()
            .token(AnalysisToken::from("track-42"))
            .source_sample_rate(
                NonZeroU32::new(consts::RATE_48).expect("invariant: a fixture rate is set"),
            )
            .revision(0)
            .build();

        assert_eq!(
            BeatGridModel::try_from(&waveform_only),
            Err(BeatGridUnavailable::NoBeats)
        );
    }

    fn downbeat_ordinals(model: &BeatGridModel) -> Vec<i64> {
        model
            .as_raw()
            .downbeats
            .iter()
            .map(|downbeat| downbeat.beat_ordinal)
            .collect()
    }

    fn meter(model: &BeatGridModel) -> Option<(u16, i64)> {
        model
            .as_raw()
            .meter
            .map(|meter| (meter.beats_per_bar.get(), meter.origin_beat_ordinal))
    }

    #[kithara::test(native, flash(false))]
    fn the_bar_the_downbeats_keep_becomes_the_meter() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..9),
            &on_beats([0, 4, 8].into_iter()),
            None,
            BeatState::Final,
        ));

        assert_eq!(downbeat_ordinals(&model), [0, 4, 8]);
        assert_eq!(meter(&model), Some((4, 0)));
    }

    /// A bar line off every beat casts no vote, and the one left measures no
    /// bar: the pass states no bars rather than a phase nothing repeats.
    #[kithara::test(native, flash(false))]
    fn one_placed_bar_line_measures_no_bar() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..9),
            &[0, 30_000],
            None,
            BeatState::Provisional,
        ));

        assert!(model.as_raw().downbeats.is_empty());
        assert_eq!(model.as_raw().meter, None);
        assert_eq!(
            model.as_raw().beats.len(),
            9,
            "the beats themselves still stand"
        );
    }

    /// A detector hears a bar line on the wrong beat now and then; the bars
    /// around it outvote it instead of withdrawing the whole grid.
    #[kithara::test(native, flash(false))]
    fn a_bar_line_on_the_wrong_beat_is_outvoted() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..21),
            &on_beats([0, 4, 8, 10, 12, 16, 20].into_iter()),
            None,
            BeatState::Final,
        ));

        assert_eq!(downbeat_ordinals(&model), [0, 4, 8, 12, 16, 20]);
        assert_eq!(meter(&model), Some((4, 0)));
    }

    /// A bar the detector skipped is still a bar: the phase the others agree
    /// on states it, claiming no confidence of its own.
    #[kithara::test(native, flash(false))]
    fn a_bar_the_detector_skipped_is_stated_on_the_agreed_phase() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..17),
            &on_beats([0, 4, 12, 16].into_iter()),
            None,
            BeatState::Final,
        ));

        assert_eq!(downbeat_ordinals(&model), [0, 4, 8, 12, 16]);
        assert_eq!(
            model
                .as_raw()
                .downbeats
                .iter()
                .map(|downbeat| downbeat.confidence)
                .collect::<Vec<_>>(),
            [Some(0.9), Some(0.9), None, Some(0.9), Some(0.9)]
        );
    }

    /// Bar lines split evenly between two phases prove neither.
    #[kithara::test(native, flash(false))]
    fn bar_lines_split_between_two_phases_state_no_bar() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..15),
            &on_beats([0, 4, 10, 14].into_iter()),
            None,
            BeatState::Final,
        ));

        assert!(model.as_raw().downbeats.is_empty());
        assert_eq!(model.as_raw().meter, None);
    }

    /// The grid stops where the media does, and a marker past it is no
    /// observation of the music.
    #[kithara::test(native, flash(false))]
    fn a_marker_past_the_stated_length_is_dropped_rather_than_published() {
        let model = grid(&analysis(
            consts::RATE_48,
            &on_beats(0..12),
            &[],
            Some(9 * consts::PERIOD_48),
            BeatState::Final,
        ));

        assert_eq!(ordinals(&model), (0..=9).collect::<Vec<_>>());
        assert_eq!(heard(&model), (0..=9).collect::<Vec<_>>());
    }
}
