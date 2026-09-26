use std::collections::BTreeMap;

/// The bar a run of heard bar lines agrees on, as beats per bar and the phase
/// the bar lines take within it, given the beat ordinal each one fell on.
///
/// The bar is the one spacing heard more often than any other, and the phase
/// is the remainder more than half the bar lines share. A detector puts the odd
/// bar line on the wrong beat and skips others; the rest outvote it. A tie on
/// either count proves nothing, so the run states no bar.
pub(crate) fn voted_bar(ordinals: impl Iterator<Item = i64> + Clone) -> Option<(i64, i64)> {
    let bar = sole_mode(
        ordinals
            .clone()
            .zip(ordinals.clone().skip(1))
            .map(|(from, to)| to - from),
    )?;
    if bar <= 0 {
        return None;
    }
    let phase = sole_mode(ordinals.clone().map(|ordinal| ordinal.rem_euclid(bar)))?;
    let (agreeing, votes) = ordinals.fold((0_usize, 0_usize), |(agreeing, votes), ordinal| {
        (
            agreeing + usize::from(ordinal.rem_euclid(bar) == phase),
            votes + 1,
        )
    });
    (2 * agreeing > votes).then_some((bar, phase))
}

fn sole_mode(values: impl Iterator<Item = i64>) -> Option<i64> {
    let mut counts = BTreeMap::<i64, usize>::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    let most = counts.values().copied().max()?;
    let mut modes = counts.into_iter().filter(|(_, count)| *count == most);
    let (mode, _) = modes.next()?;
    modes.next().is_none().then_some(mode)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::voted_bar;

    #[kithara::test]
    fn a_bar_line_on_the_wrong_beat_is_outvoted() {
        assert_eq!(
            voted_bar([0, 4, 8, 10, 12, 16, 20].into_iter()),
            Some((4, 0))
        );
    }

    #[kithara::test]
    fn a_skipped_bar_does_not_move_the_bar() {
        assert_eq!(voted_bar([1, 5, 13, 17].into_iter()), Some((4, 1)));
    }

    #[kithara::test]
    fn two_phases_heard_equally_state_no_bar() {
        assert_eq!(voted_bar([0, 4, 10, 14].into_iter()), None);
    }

    #[kithara::test]
    fn one_bar_line_states_no_bar() {
        assert_eq!(voted_bar([4].into_iter()), None);
    }
}
