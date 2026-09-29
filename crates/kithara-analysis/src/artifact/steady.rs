use std::num::NonZeroU32;

use bon::Builder;
use kithara_derive::Patch;
use kithara_platform::time::Duration;
use num_traits::cast::{AsPrimitive, ToPrimitive};

/// How steady a pass's beat markers must be for it to state a grid, and how
/// far a marker may sit from that grid and still be one of its beats.
///
/// A grid is one straight line of whole beats: the longest run of markers
/// that stay within [`Self::residual`] of the least-squares line through
/// them. A run shorter than [`Self::min_run_beats`], holding less than
/// [`Self::min_coverage`] of the markers, or beside a stretch of markers that
/// keeps another tempo, states no grid: the track keeps no one tempo long
/// enough to be followed on one.
#[derive(Builder, Clone, Copy, Debug, PartialEq, Patch)]
#[non_exhaustive]
#[derive(kithara_derive::BuiltDefault)]
pub struct GridFit {
    /// How far a marker may sit from the line and still be one of its beats.
    #[builder(default = Duration::from_millis(25))]
    #[patch(humantime)]
    pub residual: Duration,
    /// The fewest markers a steady run holds.
    #[builder(default = NonZeroU32::new(8).unwrap_or(NonZeroU32::MIN))]
    pub min_run_beats: NonZeroU32,
    /// The share of the markers a steady run holds.
    #[builder(default = MarkerShare::DEFAULT)]
    pub min_coverage: MarkerShare,
}

/// A share of a pass's beat markers, from none of them to all.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, kithara_derive::Ranged)]
#[ranged(min = 0.0, max = 1.0, default = 0.5)]
pub struct MarkerShare(f64);

/// Beat `k` of a steady run, `origin + k * period` seconds into the track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Line {
    pub(super) origin: f64,
    pub(super) period: f64,
}

impl Line {
    pub(super) fn at(self, ordinal: i64) -> f64 {
        let ordinal: f64 = ordinal.as_();
        ordinal.mul_add(self.period, self.origin)
    }

    /// The whole beat of this line nearest `at`.
    pub(super) fn nearest(self, at: f64) -> Option<i64> {
        ((at - self.origin) / self.period).round().to_i64()
    }

    /// The whole beat of this line within `residual` of `at`, if there is one.
    pub(super) fn beat_of(self, at: f64, residual: f64) -> Option<i64> {
        self.nearest(at)
            .filter(|&ordinal| (at - self.at(ordinal)).abs() <= residual)
    }
}

/// The markers one line holds, each with the beat of the line it names.
#[derive(Debug)]
pub(super) struct SteadyRun {
    pub(super) line: Line,
    /// `(ordinal, marker index)` in marker order; ordinals strictly rise.
    pub(super) members: Vec<(i64, usize)>,
}

impl SteadyRun {
    /// This run counted from the beat of its line nearest the start of the
    /// track, so a refit that moves the line by a hair keeps every number
    /// unless its beats stand half a beat either side of the start.
    fn numbered_from_start(self) -> Self {
        let zero = self.line.nearest(0.0).unwrap_or_default();
        Self {
            line: Line {
                origin: self.line.at(zero),
                period: self.line.period,
            },
            members: self
                .members
                .into_iter()
                .map(|(ordinal, index)| (ordinal - zero, index))
                .collect(),
        }
    }
}

/// Running least-squares sums of `(ordinal, seconds)` pairs.
#[derive(Clone, Copy, Default)]
struct Sums {
    count: f64,
    ordinals: f64,
    times: f64,
    ordinal_squares: f64,
    products: f64,
}

impl Sums {
    fn add(&mut self, ordinal: i64, at: f64) {
        let ordinal: f64 = ordinal.as_();
        self.count += 1.0;
        self.ordinals += ordinal;
        self.times += at;
        self.ordinal_squares += ordinal * ordinal;
        self.products += ordinal * at;
    }

    /// The line through the pairs: at `period` when one is given, otherwise
    /// the one whose slope fits them best.
    fn line(&self, period: Option<f64>) -> Line {
        let spread = self
            .count
            .mul_add(self.ordinal_squares, -self.ordinals * self.ordinals);
        let period = period.unwrap_or_else(|| {
            self.count
                .mul_add(self.products, -self.ordinals * self.times)
                / spread
        });
        Line {
            origin: period.mul_add(-self.ordinals, self.times) / self.count,
            period,
        }
    }
}

impl GridFit {
    /// The longest steady run through `times`, ascending marker seconds, with
    /// beats first counted at `period` seconds apart and numbered from the
    /// beat nearest the start of the track; `None` when no run is long enough
    /// to state a grid, holds too few of the markers, or the track keeps
    /// another tempo for a stretch of them.
    ///
    /// A run is grown from every marker in turn, since a later one can start
    /// a longer line than an earlier one whose shorter line took part of it,
    /// until no later marker has enough markers left to beat the longest.
    pub(super) fn steady_run(&self, times: &[f64], period: f64) -> Option<SteadyRun> {
        let least = self.run_beats().max(self.share(times.len()));
        let mut best: Option<SteadyRun> = None;
        for start in 0..times.len() {
            let left = times.len() - start;
            if left < least || best.as_ref().is_some_and(|best| best.members.len() >= left) {
                break;
            }
            let run = self.grow(times, start, period);
            if best
                .as_ref()
                .is_none_or(|best| run.members.len() > best.members.len())
            {
                best = Some(run);
            }
        }
        best.filter(|run| run.members.len() >= least && !self.contradicted(times, run.line))
            .map(SteadyRun::numbered_from_start)
    }

    fn run_beats(&self) -> usize {
        self.min_run_beats.get().to_usize().unwrap_or(usize::MAX)
    }

    /// The fewest of `markers` that are [`Self::min_coverage`] of them.
    fn share(&self, markers: usize) -> usize {
        let markers: f64 = markers.as_();
        (f64::from(self.min_coverage) * markers)
            .ceil()
            .to_usize()
            .unwrap_or(usize::MAX)
    }

    /// Whether the track keeps, for [`Self::min_run_beats`] markers in a
    /// row, a steady tempo that no line at a metrical level of `line`'s own
    /// could hold: half its period, its period, or a whole number of them.
    /// A phrase tracked on the off-beats, a passage tracked on the eighths
    /// and beats the tracker missed all keep the line's tempo; a turn to
    /// another tempo does not, however many of its markers the line shares.
    ///
    /// Each stretch starts at the marker the one before it ended on, so the
    /// markers are walked once.
    fn contradicted(&self, times: &[f64], line: Line) -> bool {
        let least = self.run_beats();
        let mut start = 0;
        while start + 1 < times.len() {
            let stretch = self.stretch(times, start);
            if stretch.members.len() >= least && !self.at_a_level(times, &stretch, line.period) {
                return true;
            }
            start = stretch
                .members
                .last()
                .map_or(start + 1, |&(_, index)| index.max(start + 1));
        }
        false
    }

    /// The markers from `start` on that one line holds with none between
    /// them left out: beats first stand the interval between the first two
    /// markers apart, then the line is the one that fits them best, and the
    /// stretch ends at the first marker off it.
    fn stretch(&self, times: &[f64], start: usize) -> SteadyRun {
        let residual = self.residual.as_secs_f64();
        let mut sums = Sums::default();
        sums.add(0, times[start]);
        let mut members = vec![(0, start)];
        let mut line = Line {
            origin: times[start],
            period: times[start + 1] - times[start],
        };
        for (index, &at) in times.iter().enumerate().skip(start + 1) {
            let Some(ordinal) = line
                .beat_of(at, residual)
                .filter(|&ordinal| members.last().is_some_and(|&(last, _)| ordinal > last))
            else {
                break;
            };
            sums.add(ordinal, at);
            members.push((ordinal, index));
            line = sums.line(None);
        }
        SteadyRun { line, members }
    }

    /// Whether the line at a metrical level of `period` near the stretch's
    /// own that fits the stretch best holds [`Self::min_coverage`] of its
    /// markers within [`Self::residual`], counting the stretch's beats as its
    /// own. A marker the tracker set off the beat costs the stretch that
    /// marker, not its tempo.
    fn at_a_level(&self, times: &[f64], stretch: &SteadyRun, period: f64) -> bool {
        let residual = self.residual.as_secs_f64();
        let least = self.share(stretch.members.len());
        let mut sums = Sums::default();
        for &(ordinal, index) in &stretch.members {
            sums.add(ordinal, times[index]);
        }
        let multiple = stretch.line.period / period;
        let half = (multiple < 1.0).then_some(0.5);
        let whole = [multiple.floor(), multiple.ceil()]
            .into_iter()
            .filter(|&level| level >= 1.0);
        half.into_iter().chain(whole).any(|level| {
            let line = sums.line(Some(level * period));
            let held = stretch
                .members
                .iter()
                .filter(|&&(ordinal, index)| (times[index] - line.at(ordinal)).abs() <= residual)
                .count();
            held >= least
        })
    }

    /// The run a line through the marker at `start` gathers from there on.
    ///
    /// Each later marker joins when it sits within [`Self::residual`] of a
    /// whole beat after the last one the line holds. Until the run holds
    /// [`Self::min_run_beats`] its beats keep `period` apart, since a slope
    /// read off a few markers is mostly their jitter, and never fewer than
    /// two, which state no slope at all; from then on the line
    /// is the one that fits them best. The markers the finished line no
    /// longer holds are let go and the line is fitted again without them,
    /// until it holds every marker left.
    fn grow(&self, times: &[f64], start: usize, period: f64) -> SteadyRun {
        let residual = self.residual.as_secs_f64();
        let settled = self.run_beats().max(2);
        let slope = |members: usize| (members < settled).then_some(period);
        let mut sums = Sums::default();
        sums.add(0, times[start]);
        let mut members = vec![(0, start)];
        let mut line = sums.line(Some(period));
        for (index, &at) in times.iter().enumerate().skip(start + 1) {
            let Some(ordinal) = line.beat_of(at, residual) else {
                continue;
            };
            if members.last().is_some_and(|&(last, _)| ordinal <= last) {
                continue;
            }
            sums.add(ordinal, at);
            members.push((ordinal, index));
            line = sums.line(slope(members.len()));
        }
        loop {
            let held = members.len();
            members.retain(|&(ordinal, index)| (times[index] - line.at(ordinal)).abs() <= residual);
            if members.len() == held || members.is_empty() {
                break;
            }
            let mut sums = Sums::default();
            for &(ordinal, index) in &members {
                sums.add(ordinal, times[index]);
            }
            line = sums.line(slope(members.len()));
        }
        SteadyRun { line, members }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::GridFit;
    use crate::consts;

    fn fit() -> GridFit {
        GridFit::default()
    }

    /// Markers read off a 20 ms model frame land up to 10 ms either side of
    /// the beat; the line through them is the beat itself.
    #[kithara::test(native)]
    fn a_line_through_jittered_markers_lands_on_the_beats() {
        let period = 0.483_87;
        let origin = 0.237;
        let times: Vec<f64> = (0..40)
            .map(|beat| {
                let exact = f64::from(beat).mul_add(period, origin);
                (exact / 0.02).round() * 0.02
            })
            .collect();

        let run = fit().steady_run(&times, 0.48).expect("a steady run");

        assert_eq!(run.members.len(), 40);
        assert!(
            (run.line.period - period).abs() < 1e-4,
            "the fitted period is the music's: {:?}",
            run.line
        );
        assert!(
            (run.line.origin - origin).abs() < 2e-3,
            "the fitted phase is the music's: {:?}",
            run.line
        );
    }

    #[kithara::test(native)]
    fn a_run_shorter_than_the_minimum_states_no_grid() {
        let times: Vec<f64> = (0..7)
            .map(|beat| f64::from(beat) * consts::BEAT_SECONDS)
            .collect();

        assert!(fit().steady_run(&times, consts::BEAT_SECONDS).is_none());
    }

    /// A track that turns to another steady tempo contradicts one line
    /// through it, however many markers the first tempo holds and however
    /// many of the new one land on its line by chance.
    #[kithara::test(native)]
    fn a_tempo_turning_inside_the_run_states_no_grid() {
        let steady = (0..16).map(|beat| f64::from(beat) * 0.5);
        let turned = (0..12).map(|beat| f64::from(beat).mul_add(0.6, 8.0));
        let times: Vec<f64> = steady.chain(turned).collect();

        assert!(fit().steady_run(&times, 0.5).is_none());
    }

    /// A tempo the track turns to contradicts the line although the line
    /// holds some of its markers: every fifth beat of 96 BPM is every
    /// fourth of 120.
    #[kithara::test(native)]
    fn a_turn_whose_markers_the_old_line_shares_states_no_grid() {
        let steady = (0..16).map(|beat| f64::from(beat) * 0.5);
        let turned = (0..8).map(|beat| f64::from(beat).mul_add(0.625, 8.0));
        let times: Vec<f64> = steady.chain(turned).collect();

        assert!(fit().steady_run(&times, 0.5).is_none());
    }

    /// A tempo the tracker follows on every other beat is still another
    /// tempo, however far apart its markers stand.
    #[kithara::test(native)]
    fn a_turn_tracked_on_every_other_beat_states_no_grid() {
        let steady = (0..32).map(|beat| f64::from(beat) * 0.5);
        let turned = (0..20).map(|beat| f64::from(beat).mul_add(1.25, 16.0));
        let times: Vec<f64> = steady.chain(turned).collect();

        assert!(fit().steady_run(&times, 0.5).is_none());
    }

    /// A passage tracked on the eighths keeps the tempo of the beats around
    /// it; the line holds the beats among them.
    #[kithara::test(native)]
    fn a_passage_tracked_at_double_time_keeps_the_grid() {
        let before = (0..16).map(|beat| f64::from(beat) * 0.5);
        let eighths = (0..16).map(|eighth| f64::from(eighth).mul_add(0.25, 8.0));
        let after = (0..16).map(|beat| f64::from(beat).mul_add(0.5, 12.0));
        let times: Vec<f64> = before.chain(eighths).chain(after).collect();

        let run = fit().steady_run(&times, 0.5).expect("a steady run");

        assert_eq!(run.members.len(), 40);
    }

    /// Beats the tracker missed leave gaps of whole beats in one tempo.
    #[kithara::test(native)]
    fn beats_the_tracker_missed_keep_the_grid() {
        let times: Vec<f64> = (0..60_u32)
            .filter(|beat| beat % 7 != 3 && beat % 11 != 5)
            .map(|beat| f64::from(beat) * 0.5)
            .collect();

        let run = fit().steady_run(&times, 0.5).expect("a steady run");

        assert_eq!(run.members.len(), times.len());
    }

    /// A tracker that settles onto the kick over a few beats sets the first
    /// markers of a stretch late; the stretch keeps the tempo of the line,
    /// and the line lets go of the marker it does not hold.
    #[kithara::test(native)]
    fn a_stretch_the_tracker_settles_into_keeps_the_grid() {
        let late = [0.055, 0.024, 0.015, 0.014, 0.011, 0.002];
        let times: Vec<f64> = (0..40_u32)
            .zip(late.into_iter().chain(std::iter::repeat(0.0)))
            .map(|(beat, lag)| f64::from(beat).mul_add(0.5, lag))
            .collect();

        let run = fit().steady_run(&times, 0.5).expect("a steady run");

        assert_eq!(run.members.len(), times.len() - 1);
    }

    /// A run that starts at a later marker than the one before it is the
    /// run, although the earlier marker's shorter line took some of it.
    #[kithara::test(native)]
    fn a_run_starting_inside_a_shorter_one_is_found() {
        let times = [
            0.195, 0.720, 1.235, 1.715, 2.205, 2.740, 3.210, 3.735, 4.205,
        ];

        let run = fit().steady_run(&times, 0.5).expect("a steady run");

        assert_eq!(run.members.len(), 8);
    }

    /// Letting go of the markers a fitted line does not hold moves the line
    /// again; the run is the markers the line it settles on holds.
    #[kithara::test(native)]
    fn every_member_of_a_run_sits_within_the_residual_of_its_line() {
        let times: Vec<f64> = [
            10_448, 33_792, 57_038, 80_969, 106_749, 130_453, 151_891, 177_103, 200_604, 223_849,
            248_251,
        ]
        .into_iter()
        .map(|frame: u32| f64::from(frame) / 48_000.0)
        .collect();
        let fit = fit();

        let run = fit.steady_run(&times, 0.5).expect("a steady run");

        let worst = run
            .members
            .iter()
            .map(|&(ordinal, index)| (times[index] - run.line.at(ordinal)).abs())
            .fold(0.0, f64::max);
        assert!(
            worst <= fit.residual.as_secs_f64(),
            "a member {:.3} ms off its line",
            worst * 1e3
        );
    }

    /// A track that holds one tempo for a third of its markers and wanders
    /// for the rest keeps no tempo a player could follow.
    #[kithara::test(native)]
    fn a_run_holding_too_few_of_the_markers_states_no_grid() {
        let steady = (0..10).map(|beat| f64::from(beat) * consts::BEAT_SECONDS);
        let wandering = (0..20).map(|step| 10.0 + f64::from(step) * consts::BEAT_SECONDS * 1.3);
        let times: Vec<f64> = steady.chain(wandering).collect();

        assert!(fit().steady_run(&times, consts::BEAT_SECONDS).is_none());
    }
}
