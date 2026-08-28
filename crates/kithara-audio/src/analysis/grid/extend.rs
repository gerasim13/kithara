use num_traits::cast::ToPrimitive;

use crate::waveform::{BeatGrid, MarkedBeat};

const FILL_THRESHOLD: f64 = 1.5;

pub(crate) fn extend_over(grid: BeatGrid, extent: u64, source_rate: u32) -> BeatGrid {
    let Some(beat) = beat_period(grid.bpm(), source_rate) else {
        return grid;
    };
    let beats = marked(
        spread(grid.beats(), beat, extent),
        grid.beats(),
        grid.beat_confidence(),
    );
    let bar = bar_period(grid.downbeats(), beat);
    let downbeats = marked(
        spread(grid.downbeats(), bar, extent),
        grid.downbeats(),
        grid.downbeat_confidence(),
    );

    BeatGrid::new(grid.bpm(), beats, downbeats, grid.segments().to_vec())
}

fn beat_period(bpm: f64, source_rate: u32) -> Option<f64> {
    if bpm <= 0.0 {
        return None;
    }
    Some(60.0 / bpm * f64::from(source_rate))
}

fn bar_period(downbeats: &[u64], beat: f64) -> f64 {
    let observed = downbeats
        .windows(2)
        .filter_map(|pair| pair[1].checked_sub(pair[0]))
        .filter_map(|gap| gap.to_f64())
        .find(|gap| *gap > 0.0);
    let Some(gap) = observed else {
        return beat;
    };
    (gap / beat).round().max(1.0) * beat
}

fn marked(spread: Vec<u64>, detected: &[u64], confidence: &[Option<f32>]) -> Vec<MarkedBeat> {
    spread
        .into_iter()
        .map(|frame| {
            let known = detected
                .binary_search(&frame)
                .ok()
                .and_then(|index| confidence.get(index).copied())
                .flatten();
            (frame, known)
        })
        .collect()
}

fn spread(marks: &[u64], period: f64, extent: u64) -> Vec<u64> {
    if period <= 0.0 {
        return marks.to_vec();
    }
    let Some((first, last)) = marks.first().zip(marks.last()) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(marks.len());
    for at in walk_back(*first, period) {
        out.push(at);
    }
    out.reverse();

    for pair in marks.windows(2) {
        out.push(pair[0]);
        fill_between(&mut out, pair[0], pair[1], period);
    }
    out.push(*last);

    let mut step = 1.0;
    while let Some(at) = offset(*last, step * period) {
        if at >= extent {
            break;
        }
        out.push(at);
        step += 1.0;
    }
    out
}

fn walk_back(first: u64, period: f64) -> Vec<u64> {
    let mut out = Vec::new();
    let Some(anchor) = first.to_f64() else {
        return out;
    };
    let mut step = 1.0;
    while anchor - step * period >= 0.0 {
        if let Some(at) = (anchor - step * period).to_u64() {
            out.push(at);
        }
        step += 1.0;
    }
    out
}

fn fill_between(out: &mut Vec<u64>, from: u64, to: u64, period: f64) {
    let Some(gap) = to.checked_sub(from).and_then(|gap| gap.to_f64()) else {
        return;
    };
    let steps = (gap / period).round();
    if steps < FILL_THRESHOLD {
        return;
    }
    let Some(anchor) = from.to_f64() else {
        return;
    };
    let stride = gap / steps;
    let mut step = 1.0;
    while step < steps {
        if let Some(at) = (anchor + step * stride).to_u64() {
            out.push(at);
        }
        step += 1.0;
    }
}

fn offset(from: u64, by: f64) -> Option<u64> {
    from.to_f64().and_then(|at| (at + by).to_u64())
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::extend_over;
    use crate::waveform::BeatGrid;

    const RATE: u32 = 44_100;

    fn grid(beats: Vec<u64>) -> BeatGrid {
        let downbeats = beats.iter().step_by(4).map(detected).collect();
        BeatGrid::new(
            120.0,
            beats.iter().map(detected).collect(),
            downbeats,
            Vec::new(),
        )
    }

    fn detected(frame: &u64) -> (u64, Option<f32>) {
        (*frame, Some(0.9))
    }

    #[kithara::test]
    fn an_extrapolated_marker_claims_nothing_and_a_detected_one_keeps_its_answer() {
        let detected = vec![0, 22_050, 44_100, 66_150];
        let out = extend_over(grid(detected.clone()), 10 * u64::from(RATE), RATE);

        assert!(
            out.beats().len() > detected.len(),
            "the grid reached past what was detected"
        );
        for (&frame, &confidence) in out.beats().iter().zip(out.beat_confidence()) {
            if detected.contains(&frame) {
                assert_eq!(
                    confidence,
                    Some(0.9),
                    "a detected marker keeps what the detector said about it"
                );
            } else {
                assert_eq!(
                    confidence, None,
                    "a marker at {frame} nothing detected claims no confidence"
                );
            }
        }
    }

    #[kithara::test]
    fn a_short_run_of_markers_covers_the_whole_extent() {
        // Four beats near the start of a ten-second track.
        let detected = vec![0, 22_050, 44_100, 66_150];
        let out = extend_over(grid(detected.clone()), 10 * u64::from(RATE), RATE);

        for beat in &detected {
            assert!(
                out.beats().contains(beat),
                "a detected marker must survive: {beat}"
            );
        }
        assert!(
            out.beats().len() >= 19,
            "ten seconds at 120 bpm is about twenty beats, got {}",
            out.beats().len()
        );
        assert!(
            out.beats().windows(2).all(|pair| pair[1] > pair[0]),
            "markers must stay ascending"
        );
        assert!(
            out.beats().last().is_some_and(|last| *last < 441_000),
            "extrapolation must stop at the extent"
        );
    }

    #[kithara::test]
    fn markers_before_the_first_detection_are_filled_in() {
        // The first covered piece starts two seconds in.
        let out = extend_over(grid(vec![88_200, 110_250]), 5 * u64::from(RATE), RATE);
        assert!(
            out.beats().first().is_some_and(|first| *first < 88_200),
            "the run before the first detection must be filled: {:?}",
            out.beats().first()
        );
    }

    #[kithara::test]
    fn a_gap_between_detections_is_divided_evenly() {
        // Two detected pieces four beats apart.
        let out = extend_over(grid(vec![0, 22_050, 110_250, 132_300]), 132_300, RATE);
        assert_eq!(
            out.beats(),
            &[0, 22_050, 44_100, 66_150, 88_200, 110_250, 132_300],
            "the gap must be divided at the observed period"
        );
    }

    #[kithara::test]
    fn a_grid_with_nothing_to_go_on_is_left_alone() {
        let empty = BeatGrid::new(120.0, Vec::new(), Vec::new(), Vec::new());
        assert!(extend_over(empty, 441_000, RATE).beats().is_empty());

        let zero_tempo = BeatGrid::new(0.0, vec![(100, Some(0.9))], Vec::new(), Vec::new());
        assert_eq!(
            extend_over(zero_tempo, 441_000, RATE).beats(),
            &[100],
            "a single marker without a tempo cannot be spread"
        );
    }
}
