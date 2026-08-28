use kithara_decode::PcmMeta;

/// Half-open range of decoded source frames, `[start, end)`.
///
/// Read off the chunk's own metadata: `PcmMeta::frame_offset` is absolute from
/// the start of the track and a seek landing rewrites it to the landed frame,
/// so a range never depends on arrival order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRange {
    end: u64,
    start: u64,
}

impl FrameRange {
    #[must_use]
    pub const fn new(start: u64, frames: u64) -> Self {
        Self {
            end: start.saturating_add(frames),
            start,
        }
    }

    #[must_use]
    pub const fn frames(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }

    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }
}

impl From<&PcmMeta> for FrameRange {
    fn from(meta: &PcmMeta) -> Self {
        Self::new(meta.frame_offset, u64::from(meta.frames))
    }
}

/// Source frame ranges the pass has observed, kept as sorted, disjoint,
/// non-adjacent runs. Two runs that touch are one run: a window is evaluable
/// exactly when it fits inside a single run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Coverage {
    runs: Vec<FrameRange>,
}

impl Coverage {
    /// Whether `range` sits inside one contiguous run.
    #[must_use]
    pub fn contains(&self, range: FrameRange) -> bool {
        range.is_empty()
            || self
                .runs
                .iter()
                .any(|run| run.start <= range.start && range.end <= run.end)
    }

    /// Total covered frames, counting an overlap once.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.runs
            .iter()
            .fold(0, |sum, run| sum.saturating_add(run.frames()))
    }

    /// Add `range`, merging it with every run it touches.
    pub fn insert(&mut self, range: FrameRange) {
        if range.is_empty() {
            return;
        }

        let first = self.runs.partition_point(|run| run.end < range.start);
        let last = self.runs.partition_point(|run| run.start <= range.end);
        let Some(overlapped) = self.runs.get(first..last).filter(|runs| !runs.is_empty()) else {
            self.runs.insert(first, range);
            return;
        };

        let merged = FrameRange {
            end: overlapped
                .iter()
                .fold(range.end, |end, run| end.max(run.end)),
            start: overlapped
                .iter()
                .fold(range.start, |start, run| start.min(run.start)),
        };
        self.runs.splice(first..last, [merged]);
    }

    /// Highest covered source frame. At end of stream this is the source
    /// length, which is how a pass learns its extent without a duration.
    #[must_use]
    pub fn frontier(&self) -> u64 {
        self.runs.last().map_or(0, |run| run.end())
    }

    /// The ranges of `[0, horizon)` this coverage does not hold, in source
    /// order.
    ///
    /// Nothing beyond `horizon` is reported: a gap can only be named where
    /// something is known to exist.
    #[must_use]
    pub fn gaps(&self, horizon: u64) -> Vec<FrameRange> {
        let mut out = Vec::new();
        let mut at = 0;
        for run in &self.runs {
            if run.start() >= horizon {
                break;
            }
            if run.start() > at {
                out.push(FrameRange::new(at, run.start() - at));
            }
            at = at.max(run.end());
        }
        if at < horizon {
            out.push(FrameRange::new(at, horizon - at));
        }
        out
    }

    /// The observed ranges, in source order, disjoint and non-adjacent.
    #[must_use]
    pub fn runs(&self) -> &[FrameRange] {
        &self.runs
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_decode::{PcmMeta, PcmSpec};
    use kithara_test_utils::kithara;

    use super::{Coverage, FrameRange};

    fn meta(frame_offset: u64, frames: u32) -> PcmMeta {
        PcmMeta {
            spec: PcmSpec {
                channels: 2,
                sample_rate: NonZeroU32::new(44_100).unwrap_or(NonZeroU32::MIN),
            },
            frame_offset,
            frames,
            ..Default::default()
        }
    }

    fn covered(coverage: &Coverage, start: u64, frames: u64) -> bool {
        coverage.contains(FrameRange::new(start, frames))
    }

    #[kithara::test]
    fn adjacent_chunks_meet_exactly() {
        assert_eq!(FrameRange::from(&meta(0, 1024)), FrameRange::new(0, 1024));
        assert_eq!(
            FrameRange::from(&meta(1024, 1024)),
            FrameRange::new(1024, 1024),
            "the second chunk starts where the first ends"
        );

        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::from(&meta(0, 1024)));
        coverage.insert(FrameRange::from(&meta(1024, 1024)));
        assert!(covered(&coverage, 0, 2048), "touching runs merge");
        assert_eq!(coverage.frames(), 2048);
    }

    #[kithara::test]
    fn range_follows_a_seek_landing() {
        // A seek rewrites `frame_offset` to the landed source frame; the range
        // follows it instead of continuing the previous chunk.
        let landed = FrameRange::from(&meta(441_000, 1024));
        assert_eq!(landed, FrameRange::new(441_000, 1024));

        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::from(&meta(0, 1024)));
        coverage.insert(landed);
        assert_eq!(coverage.frames(), 2048, "the skipped span stays uncovered");
        assert!(!covered(&coverage, 0, 442_024));
    }

    #[kithara::test]
    fn duplicate_insert_changes_nothing() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 500));
        coverage.insert(FrameRange::new(0, 500));
        assert_eq!(coverage.frames(), 500, "a duplicate is counted once");
    }

    #[kithara::test]
    fn overlapping_inserts_union() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 500));
        coverage.insert(FrameRange::new(300, 500));
        assert_eq!(coverage.frames(), 800, "shared frames counted once");
        assert!(covered(&coverage, 0, 800));
    }

    #[kithara::test]
    fn gapped_inserts_stay_separate_until_filled() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 100));
        coverage.insert(FrameRange::new(300, 100));
        assert_eq!(coverage.frames(), 200);
        assert!(!covered(&coverage, 0, 400), "the gap is not covered");

        coverage.insert(FrameRange::new(100, 200));
        assert_eq!(coverage.frames(), 400, "filling the gap joins the runs");
        assert!(covered(&coverage, 0, 400));
    }

    #[kithara::test]
    fn shuffled_inserts_yield_the_same_coverage() {
        let ranges = [
            FrameRange::new(0, 100),
            FrameRange::new(100, 100),
            FrameRange::new(200, 100),
            FrameRange::new(400, 100),
        ];
        let fill = |order: [FrameRange; 4]| {
            let mut coverage = Coverage::default();
            for range in order {
                coverage.insert(range);
            }
            coverage
        };
        let ascending = fill(ranges);
        let shuffled = fill([ranges[3], ranges[1], ranges[0], ranges[2]]);

        assert_eq!(ascending.frames(), shuffled.frames());
        assert_eq!(covered(&ascending, 0, 300), covered(&shuffled, 0, 300));
        assert_eq!(covered(&ascending, 0, 500), covered(&shuffled, 0, 500));
    }

    #[kithara::test]
    fn contains_needs_one_contiguous_run() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 100));
        coverage.insert(FrameRange::new(200, 100));
        assert!(covered(&coverage, 0, 50));
        assert!(covered(&coverage, 200, 100));
        assert!(!covered(&coverage, 50, 200), "spans the gap");
    }

    #[kithara::test]
    fn empty_range_is_ignored() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(10, 0));
        assert_eq!(coverage.frames(), 0);
        assert!(covered(&coverage, 10, 0), "nothing to cover");
    }

    fn coverage(runs: &[(u64, u64)]) -> Coverage {
        let mut out = Coverage::default();
        for (start, frames) in runs {
            out.insert(FrameRange::new(*start, *frames));
        }
        out
    }

    #[kithara::test]
    fn a_hole_between_runs_is_a_gap() {
        let gaps = coverage(&[(0, 100), (300, 100)]).gaps(400);
        assert_eq!(gaps, vec![FrameRange::new(100, 200)]);
    }

    #[kithara::test]
    fn the_tail_below_the_horizon_is_a_gap() {
        let gaps = coverage(&[(0, 100)]).gaps(250);
        assert_eq!(gaps, vec![FrameRange::new(100, 150)]);
    }

    #[kithara::test]
    fn a_run_starting_past_the_start_leaves_the_head_missing() {
        let gaps = coverage(&[(50, 100)]).gaps(150);
        assert_eq!(gaps, vec![FrameRange::new(0, 50)]);
    }

    #[kithara::test]
    fn nothing_is_reported_beyond_the_horizon() {
        // Covered to 400, but only 200 is known to exist.
        assert!(coverage(&[(0, 400)]).gaps(200).is_empty());
        // A run wholly past the horizon cannot open a gap behind it.
        assert_eq!(
            coverage(&[(0, 50), (300, 100)]).gaps(200),
            vec![FrameRange::new(50, 150)]
        );
    }

    #[kithara::test]
    fn a_run_straddling_the_horizon_closes_the_gap_before_it() {
        // The run starts below the horizon and ends past it, so the gap in
        // front of it must stop where the run does, not at the horizon.
        assert_eq!(
            coverage(&[(0, 50), (100, 200)]).gaps(200),
            vec![FrameRange::new(50, 50)]
        );
        // The same run with nothing before it leaves only the head missing.
        assert_eq!(
            coverage(&[(150, 100)]).gaps(200),
            vec![FrameRange::new(0, 150)]
        );
    }

    #[kithara::test]
    fn full_coverage_has_no_gaps() {
        assert!(coverage(&[(0, 400)]).gaps(400).is_empty());
        assert!(Coverage::default().gaps(0).is_empty());
    }

    #[kithara::test]
    fn an_empty_coverage_is_all_gap() {
        assert_eq!(
            Coverage::default().gaps(400),
            vec![FrameRange::new(0, 400)],
            "a pass that observed nothing is missing everything it knows of"
        );
    }
}
