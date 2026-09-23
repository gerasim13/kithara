use std::num::NonZeroU16;

use kithara_beat::{
    BeatGridError, BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid,
    SCHEMA_VERSION,
};
use num_traits::cast::ToPrimitive;
use thiserror::Error;

use super::{
    snapshot::{BeatSnapshot, BeatState},
    track::TrackAnalysis,
};

/// How far a marker may sit from a whole beat and still name that beat without
/// a second reading. Past a quarter beat the nearest ordinal is a guess, and a
/// guess is not an observation, so the pass publishes the tempo alone instead.
pub const ORDINAL_TOLERANCE_BEATS: f64 = 0.25;

struct Consts;

impl Consts {
    const SECONDS_PER_MINUTE: f64 = 60.0;
}

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
}

/// Restates one published pass as the server-side grid contract.
///
/// This is the only place source frames become media seconds: the artifact
/// keeps frames, and the model has no sample rate to read them against.
impl TryFrom<&TrackAnalysis> for BeatGridModel {
    type Error = BeatGridUnavailable;

    fn try_from(analysis: &TrackAnalysis) -> Result<Self, Self::Error> {
        let snapshot = analysis.beat().ok_or(BeatGridUnavailable::NoBeats)?;
        let bpm = snapshot.artifact().bpm();
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(BeatGridUnavailable::Tempo { bpm });
        }
        let rate = f64::from(analysis.source_sample_rate().get());
        let duration = analysis.extent().map(|extent| seconds(extent, rate));
        let placed = place(snapshot, rate, bpm, duration);
        let downbeats = bars(snapshot, &placed, rate, duration);
        let meter = meter(&downbeats);
        Ok(Self::try_from(RawBeatGrid {
            duration,
            bpm,
            downbeats,
            meter,
            schema_version: SCHEMA_VERSION,
            model_id: analysis.token().as_str().to_owned(),
            revision: analysis.revision(),
            state: match snapshot.state() {
                BeatState::Final => BeatGridState::Final,
                BeatState::Provisional => BeatGridState::Provisional,
            },
            beats: placed.iter().map(|(_, beat)| *beat).collect(),
        })?)
    }
}

/// The artifact's beats with the ordinal each one holds, still paired with the
/// source frame the bar lines name them by.
///
/// Ordinals count from the first marker. A marker sitting too far from a whole
/// beat to name one is left out rather than named wrongly: the grid then states
/// a gap in its numbers, which is what a consumer following ordinals reads as
/// "nothing proved here", while the beats around it stand as observed.
fn place(
    snapshot: &BeatSnapshot,
    rate: f64,
    bpm: f64,
    duration: Option<f64>,
) -> Vec<(u64, GridBeat)> {
    let artifact = snapshot.artifact();
    let period = Consts::SECONDS_PER_MINUTE / bpm;
    let Some(first) = artifact.beats().first() else {
        return Vec::new();
    };
    let origin = seconds(*first, rate);
    let mut previous: Option<i64> = None;
    artifact
        .beats()
        .iter()
        .zip(artifact.beat_confidence().iter())
        .filter_map(|(frame, confidence)| {
            let at = seconds(*frame, rate);
            if duration.is_some_and(|duration| at > duration) {
                return None;
            }
            let exact = (at - origin) / period;
            let rounded = exact.round();
            if (exact - rounded).abs() > ORDINAL_TOLERANCE_BEATS {
                return None;
            }
            let ordinal = rounded.to_i64()?;
            if previous.is_some_and(|before| ordinal <= before) {
                return None;
            }
            previous = Some(ordinal);
            Some((
                *frame,
                GridBeat {
                    at,
                    ordinal,
                    confidence: *confidence,
                },
            ))
        })
        .collect()
}

/// The bar lines, each named by the beat it falls on.
///
/// All of them or none: a bar line whose beat this grid does not carry would
/// leave the remaining ones stating a phase nothing anchors.
fn bars(
    snapshot: &BeatSnapshot,
    placed: &[(u64, GridBeat)],
    rate: f64,
    duration: Option<f64>,
) -> Vec<GridDownbeat> {
    let artifact = snapshot.artifact();
    artifact
        .downbeats()
        .iter()
        .zip(artifact.downbeat_confidence().iter())
        .filter(|(frame, _)| duration.is_none_or(|duration| seconds(**frame, rate) <= duration))
        .map(|(frame, confidence)| {
            placed
                .binary_search_by_key(frame, |(placed, _)| *placed)
                .ok()
                .map(|index| GridDownbeat {
                    at: placed[index].1.at,
                    beat_ordinal: placed[index].1.ordinal,
                    confidence: *confidence,
                })
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// The bar the downbeats prove, and nothing more: one spacing they all keep,
/// or no claim at all.
fn meter(downbeats: &[GridDownbeat]) -> Option<Meter> {
    let mut steps = downbeats
        .windows(2)
        .map(|pair| pair[1].beat_ordinal.saturating_sub(pair[0].beat_ordinal));
    let bar = steps.next()?;
    if !steps.all(|step| step == bar) {
        return None;
    }
    Some(Meter {
        beats_per_bar: NonZeroU16::new(bar.to_u16()?)?,
        origin_beat_ordinal: downbeats.first()?.beat_ordinal,
    })
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

    use super::{BeatGridUnavailable, BeatSnapshot, BeatState, TrackAnalysis};
    use crate::{BeatArtifact, artifact::track::AnalysisToken};

    struct Consts;

    impl Consts {
        const BPM: f64 = 120.0;
        /// Half a second at each rate, so the same music lands on the same
        /// seconds from two different frame counts.
        const PERIOD_44_1: u64 = 22_050;
        const PERIOD_48: u64 = 24_000;
        const RATE_44_1: u32 = 44_100;
        const RATE_48: u32 = 48_000;
    }

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
                Consts::BPM,
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
        TrackAnalysis::builder()
            .token(AnalysisToken::from("track-42"))
            .source_sample_rate(NonZeroU32::new(rate).expect("invariant: a fixture rate is set"))
            .beat(BeatSnapshot::new(artifact, state, Vec::new()))
            .maybe_extent(extent)
            .revision(7)
            .build()
    }

    fn grid(analysis: &TrackAnalysis) -> BeatGridModel {
        BeatGridModel::try_from(analysis).expect("the pass states a grid")
    }

    fn times(model: &BeatGridModel) -> Vec<(i64, f64)> {
        model
            .as_raw()
            .beats
            .iter()
            .map(|beat| (beat.ordinal, beat.at))
            .collect()
    }

    /// Detected frames become media seconds here and nowhere else.
    #[kithara::test(native, flash(false))]
    fn a_pass_publishes_its_beats_as_media_seconds_on_its_own_grid() {
        let beats: Vec<u64> = (0..4).map(|beat| beat * Consts::PERIOD_48).collect();
        let model = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[],
            Some(4 * Consts::PERIOD_48),
            BeatState::Provisional,
        ));

        assert_eq!(
            model.as_raw().model_id,
            "track-42",
            "the token names the model"
        );
        assert_eq!(model.as_raw().revision, 7);
        assert_eq!(model.as_raw().state, BeatGridState::Provisional);
        assert_eq!(model.as_raw().bpm, Consts::BPM);
        assert_eq!(
            model.as_raw().duration,
            Some(2.0),
            "the extent states the length"
        );
        assert_eq!(times(&model), [(0, 0.0), (1, 0.5), (2, 1.0), (3, 1.5)]);
    }

    /// The model carries no sample rate, so two passes over the same music
    /// must agree once their frames are read against their own rates.
    #[kithara::test(native, flash(false))]
    fn the_same_music_states_the_same_grid_from_either_source_rate() {
        let at_48: Vec<u64> = (0..4).map(|beat| beat * Consts::PERIOD_48).collect();
        let at_44_1: Vec<u64> = (0..4).map(|beat| beat * Consts::PERIOD_44_1).collect();

        assert_eq!(
            times(&grid(&analysis(
                Consts::RATE_48,
                &at_48,
                &[],
                None,
                BeatState::Final
            ))),
            times(&grid(&analysis(
                Consts::RATE_44_1,
                &at_44_1,
                &[],
                None,
                BeatState::Final
            )))
        );
    }

    /// The gap between two analysed islands costs the grid no beats.
    #[kithara::test(native, flash(false))]
    fn islands_keep_the_ordinals_the_music_gives_them() {
        let beats = [
            0,
            Consts::PERIOD_48,
            60 * Consts::PERIOD_48,
            61 * Consts::PERIOD_48,
        ];
        let model = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(
            model
                .as_raw()
                .beats
                .iter()
                .map(|beat| beat.ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 60, 61],
            "the ordinal counts beats of the music, never entries of the list"
        );
    }

    /// A pass that does not yet know the length still states what it heard.
    #[kithara::test(native, flash(false))]
    fn an_unknown_length_does_not_withhold_the_grid() {
        let model = grid(&analysis(
            Consts::RATE_48,
            &[0, Consts::PERIOD_48],
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(model.as_raw().duration, None);
        assert_eq!(model.as_raw().beats.len(), 2);
    }

    #[kithara::test(native, flash(false))]
    fn a_later_pass_publishes_the_same_grid_as_final() {
        let beats = [0, Consts::PERIOD_48];
        let provisional = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[],
            Some(2 * Consts::PERIOD_48),
            BeatState::Provisional,
        ));
        let final_pass = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[],
            Some(2 * Consts::PERIOD_48),
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
            Consts::RATE_48,
            BeatArtifact::new(0.0, vec![(0, None), (100, None)], Vec::new()),
            Some(Consts::PERIOD_48),
            BeatState::Final,
        );

        assert_eq!(
            BeatGridModel::try_from(&degraded),
            Err(BeatGridUnavailable::Tempo { bpm: 0.0 })
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_pass_without_a_beat_artifact_states_no_grid() {
        let waveform_only = TrackAnalysis::builder()
            .token(AnalysisToken::from("track-42"))
            .source_sample_rate(
                NonZeroU32::new(Consts::RATE_48).expect("invariant: a fixture rate is set"),
            )
            .revision(0)
            .build();

        assert_eq!(
            BeatGridModel::try_from(&waveform_only),
            Err(BeatGridUnavailable::NoBeats)
        );
    }

    /// A marker too far from a whole beat names none, so the grid leaves it
    /// out rather than guessing an ordinal for it; the markers that do name a
    /// beat stand as observed, with a gap where nothing was proved.
    #[kithara::test(native, flash(false))]
    fn a_marker_that_names_no_beat_is_left_out_of_the_grid() {
        let model = grid(&analysis(
            Consts::RATE_48,
            &[0, Consts::PERIOD_48, 40_000, 3 * Consts::PERIOD_48],
            &[],
            None,
            BeatState::Provisional,
        ));

        assert_eq!(model.as_raw().bpm, Consts::BPM);
        assert_eq!(
            model
                .as_raw()
                .beats
                .iter()
                .map(|beat| beat.ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 3],
            "a guessed ordinal is not an observation, and its neighbours keep theirs"
        );
    }

    #[kithara::test(native, flash(false))]
    fn the_bar_the_downbeats_keep_becomes_the_meter() {
        let beats: Vec<u64> = (0..9).map(|beat| beat * Consts::PERIOD_48).collect();
        let model = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[0, 4 * Consts::PERIOD_48, 8 * Consts::PERIOD_48],
            None,
            BeatState::Final,
        ));

        assert_eq!(
            model
                .as_raw()
                .downbeats
                .iter()
                .map(|downbeat| downbeat.beat_ordinal)
                .collect::<Vec<_>>(),
            [0, 4, 8]
        );
        assert_eq!(
            model
                .as_raw()
                .meter
                .map(|meter| (meter.beats_per_bar.get(), meter.origin_beat_ordinal)),
            Some((4, 0))
        );
    }

    /// One bar line the grid cannot place leaves the others stating a phase
    /// nothing anchors, so the pass states no bars at all.
    #[kithara::test(native, flash(false))]
    fn a_bar_line_off_the_grid_withdraws_every_bar() {
        let beats: Vec<u64> = (0..5).map(|beat| beat * Consts::PERIOD_48).collect();
        let model = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[0, 30_000],
            None,
            BeatState::Provisional,
        ));

        assert!(model.as_raw().downbeats.is_empty());
        assert_eq!(model.as_raw().meter, None);
        assert_eq!(
            model.as_raw().beats.len(),
            5,
            "the beats themselves still stand"
        );
    }

    /// Extrapolation stops where the media does.
    #[kithara::test(native, flash(false))]
    fn a_marker_past_the_stated_length_is_dropped_rather_than_published() {
        let beats: Vec<u64> = (0..4).map(|beat| beat * Consts::PERIOD_48).collect();
        let model = grid(&analysis(
            Consts::RATE_48,
            &beats,
            &[],
            Some(2 * Consts::PERIOD_48),
            BeatState::Final,
        ));

        assert_eq!(times(&model), [(0, 0.0), (1, 0.5), (2, 1.0)]);
    }
}
