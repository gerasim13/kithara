use num_traits::cast::AsPrimitive;

use super::core::GridParams;

pub(super) fn bar_gaps(db: &[f64]) -> Vec<f64> {
    db.windows(2).map(|w| w[1] - w[0]).collect()
}

/// np-style median: mean of the two middle values for even lengths.
pub(super) fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        f64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
    }
}

/// Population standard deviation (`np.std` default).
fn pop_std(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let n: f64 = values.len().as_();
    let mean = values.iter().sum::<f64>() / n;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt()
}

/// Drops detector marks closer than `min_gap` to the last retained mark.
pub(super) fn filter_close(marks: Vec<f64>, min_gap: f64) -> Vec<f64> {
    filter_close_preferring(marks, &[], min_gap)
}

/// Drops close marks while preserving a preferred mark in each collision.
pub(super) fn filter_close_preferring(
    marks: Vec<f64>,
    preferred: &[f64],
    min_gap: f64,
) -> Vec<f64> {
    let Some(&first) = marks.first() else {
        return marks;
    };
    let mut kept = vec![first];
    for &mark in &marks[1..] {
        if let Some(last) = kept.last_mut()
            && mark - *last < min_gap
        {
            let mark_is_preferred = preferred
                .binary_search_by(|candidate| candidate.total_cmp(&mark))
                .is_ok();
            let last_is_preferred = preferred
                .binary_search_by(|candidate| candidate.total_cmp(last))
                .is_ok();
            if mark_is_preferred && !last_is_preferred {
                *last = mark;
            }
            continue;
        }
        kept.push(mark);
    }
    kept
}

/// Step 3: slide a `stable_window_bars` window over bar gaps; the lowest
/// `std + |median − nominal|` window whose median sits inside the trust band
/// wins. Returns `(anchor_idx, stable_median_bar_samples)` — the window's
/// centre downbeat and its median bar length (the track's true tempo).
pub(super) fn find_stable_window(
    db: &[f64],
    nominal_bar: f64,
    params: &GridParams,
) -> Option<(usize, f64)> {
    let w = params.stable_window_bars;
    if w == 0 || db.len() < w + 1 {
        return None;
    }
    let gaps = bar_gaps(db);
    let trust_lo = nominal_bar * (1.0 - params.median_trust_ratio);
    let trust_hi = nominal_bar * (1.0 + params.median_trust_ratio);
    let mut best: Option<(f64, usize, f64)> = None;
    for start in 0..=(gaps.len() - w) {
        let window = &gaps[start..start + w];
        let med = median(window);
        if med < trust_lo || med > trust_hi {
            continue;
        }
        let score = pop_std(window) + (med - nominal_bar).abs();
        if best.is_none_or(|(s, _, _)| score < s) {
            best = Some((score, start + w / 2, med.round()));
        }
    }
    best.map(|(_, idx, med)| (idx, med))
}

/// Step 2: a downbeat is an outlier when the gap leading into it falls
/// outside the hard bar bounds or deviates from the neighbour-window median
/// factor by more than `outlier_ratio`. The first downbeat never is.
pub(super) fn classify_outliers(db: &[f64], nominal_bar: f64, params: &GridParams) -> Vec<bool> {
    let n = db.len();
    let mut outlier = vec![false; n];
    if n < 2 {
        return outlier;
    }
    let raw: Vec<Option<f64>> = bar_gaps(db)
        .iter()
        .map(|&gap| {
            (gap >= params.min_bar_ratio * nominal_bar && gap <= params.max_bar_ratio * nominal_bar)
                .then_some(gap / nominal_bar)
        })
        .collect();
    for (i, slot) in outlier.iter_mut().enumerate().skip(1) {
        let center = i - 1;
        let Some(factor) = raw[center] else {
            *slot = true;
            continue;
        };
        let lo = center.saturating_sub(params.outlier_window);
        let hi = (center + params.outlier_window + 1).min(raw.len());
        let neigh: Vec<f64> = (lo..hi)
            .filter(|&j| j != center)
            .filter_map(|j| raw[j])
            .collect();
        let med = if neigh.is_empty() {
            1.0
        } else {
            median(&neigh)
        };
        if (factor - med).abs() > params.outlier_ratio {
            *slot = true;
        }
    }
    outlier
}
