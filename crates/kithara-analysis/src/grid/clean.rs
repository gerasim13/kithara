use kithara_bufpool::{PoolError, SampleBuffer};
use num_traits::cast::AsPrimitive;

use super::{
    core::GridParams,
    scratch::{fill, retain},
};

pub(super) fn bar_gaps(db: &[f32], gaps: &mut SampleBuffer) -> Result<(), PoolError> {
    fill(gaps, db.windows(2).map(|window| window[1] - window[0]))
}

/// np-style median: mean of the two middle values for even lengths.
pub(super) fn median(values: &[f32], sorted: &mut SampleBuffer) -> Result<f64, PoolError> {
    if values.is_empty() {
        return Ok(0.0);
    }
    fill(sorted, values.iter().copied())?;
    sorted.sort_unstable_by(f32::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        Ok(f64::from(sorted[n / 2]))
    } else {
        Ok(f64::midpoint(
            f64::from(sorted[n / 2 - 1]),
            f64::from(sorted[n / 2]),
        ))
    }
}

/// Population standard deviation (`np.std` default).
fn pop_std(values: &[f32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let n: f64 = values.len().as_();
    let mean = values.iter().map(|&value| f64::from(value)).sum::<f64>() / n;
    (values
        .iter()
        .map(|&value| (f64::from(value) - mean).powi(2))
        .sum::<f64>()
        / n)
        .sqrt()
}

/// Drops detector marks closer than `min_gap` to the last retained mark.
pub(super) fn filter_close(marks: &mut SampleBuffer, min_gap: f64) {
    filter_close_preferring(marks, &[], min_gap);
}

/// Drops close marks while preserving a preferred mark in each collision.
pub(super) fn filter_close_preferring(marks: &mut SampleBuffer, preferred: &[f32], min_gap: f64) {
    if marks.len() < 2 {
        return;
    }
    let mut kept = 1;
    for read in 1..marks.len() {
        let mark = marks[read];
        let last = marks[kept - 1];
        if f64::from(mark - last) < min_gap {
            let mark_is_preferred = preferred
                .binary_search_by(|candidate| candidate.total_cmp(&mark))
                .is_ok();
            let last_is_preferred = preferred
                .binary_search_by(|candidate| candidate.total_cmp(&last))
                .is_ok();
            if mark_is_preferred && !last_is_preferred {
                marks[kept - 1] = mark;
            }
            continue;
        }
        marks[kept] = mark;
        kept += 1;
    }
    marks.truncate(kept);
}

/// Step 3: slide a `stable_window_bars` window over bar gaps; the lowest
/// `std + |median − nominal|` window whose median sits inside the trust band
/// wins. Returns `(anchor_idx, stable_median_bar_seconds)` - the window's
/// centre downbeat and its median bar length (the track's true tempo).
pub(super) fn find_stable_window(
    db: &[f32],
    nominal_bar: f64,
    params: &GridParams,
    gaps: &mut SampleBuffer,
    sorted: &mut SampleBuffer,
) -> Result<Option<(usize, f64)>, PoolError> {
    let w = params.stable_window_bars;
    if w == 0 || db.len() < w + 1 {
        return Ok(None);
    }
    bar_gaps(db, gaps)?;
    let trust_lo = nominal_bar * (1.0 - params.median_trust_ratio);
    let trust_hi = nominal_bar * (1.0 + params.median_trust_ratio);
    let mut best: Option<(f64, usize, f64)> = None;
    for start in 0..=(gaps.len() - w) {
        let window = &gaps[start..start + w];
        let med = median(window, sorted)?;
        if med < trust_lo || med > trust_hi {
            continue;
        }
        let score = pop_std(window) + (med - nominal_bar).abs();
        if best.is_none_or(|(s, _, _)| score < s) {
            best = Some((score, start + w / 2, med));
        }
    }
    Ok(best.map(|(_, idx, med)| (idx, med)))
}

/// Step 2: a downbeat is an outlier when the gap leading into it falls
/// outside the hard bar bounds or deviates from the neighbour-window median
/// factor by more than `outlier_ratio`. The first downbeat never is.
pub(super) fn classify_outliers(
    db: &[f32],
    nominal_bar: f64,
    params: &GridParams,
    outliers: &mut SampleBuffer,
    neighbors: &mut SampleBuffer,
    sorted: &mut SampleBuffer,
) -> Result<(), PoolError> {
    let n = db.len();
    fill(outliers, (0..n).map(|_| 0.0))?;
    if n < 2 {
        return Ok(());
    }
    for i in 1..n {
        let center = i - 1;
        let Some(gap) = valid_gap(db, center, nominal_bar, params) else {
            outliers[i] = 1.0;
            continue;
        };
        let lo = center.saturating_sub(params.outlier_window);
        let hi = (center + params.outlier_window + 1).min(n - 1);
        fill(
            neighbors,
            (lo..hi).map(|index| {
                (index != center)
                    .then(|| valid_gap(db, index, nominal_bar, params))
                    .flatten()
                    .unwrap_or(f32::NAN)
            }),
        )?;
        retain(neighbors, f32::is_finite);
        let median_factor = if neighbors.is_empty() {
            1.0
        } else {
            median(neighbors, sorted)? / nominal_bar
        };
        let factor = f64::from(gap) / nominal_bar;
        if (factor - median_factor).abs() > params.outlier_ratio {
            outliers[i] = 1.0;
        }
    }
    Ok(())
}

fn valid_gap(db: &[f32], index: usize, nominal_bar: f64, params: &GridParams) -> Option<f32> {
    let gap = db[index + 1] - db[index];
    let gap_seconds = f64::from(gap);
    (gap_seconds >= params.min_bar_ratio * nominal_bar
        && gap_seconds <= params.max_bar_ratio * nominal_bar)
        .then_some(gap)
}
