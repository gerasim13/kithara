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
/// [`Self::min_coverage`] of the markers, or leaving out a steady run of
/// another tempo, states no grid: the track keeps no one tempo long enough to
/// be followed on one.
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
    /// to state a grid, holds too few of the markers, or leaves out markers
    /// that keep another tempo.
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
        best.filter(|run| run.members.len() >= least && !self.contradicted(times, period, run))
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

    /// Whether the markers `run` leaves out keep a steady line of their own,
    /// a tempo the track turns to for [`Self::min_run_beats`] markers that
    /// one line through the whole track would contradict there. Each such
    /// marker starts a line at the interval to the next one, when that is
    /// from half to twice `period`: a phrase tracked on the off-beats keeps
    /// the run's tempo, and its line holds the run's own markers instead.
    fn contradicted(&self, times: &[f64], period: f64, run: &SteadyRun) -> bool {
        let mut held = vec![false; times.len()];
        for &(_, index) in &run.members {
            held[index] = true;
        }
        let tempo = period / 2.0..=period * 2.0;
        let least = self.run_beats();
        times.windows(2).enumerate().any(|(start, pair)| {
            let local = pair[1] - pair[0];
            !held[start]
                && tempo.contains(&local)
                && self
                    .grow(times, start, local)
                    .members
                    .iter()
                    .filter(|&&(_, index)| !held[index])
                    .count()
                    >= least
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
