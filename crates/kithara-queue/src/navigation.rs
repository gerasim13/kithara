use std::collections::VecDeque;

/// Behavior when the queue reaches the last track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepeatMode {
    /// Stop after the last track; [`NavigationState::next`] returns `None`.
    #[default]
    Off,
    /// Repeat the currently selected track.
    One,
    /// Loop back to the first track.
    All,
}

/// Pure-logic navigation state: current index, history, shuffle, repeat.
///
/// Mirrors `kithara-app::playlist::PlaylistState`. Caller owns locking;
/// methods take `&mut self` so the surrounding [`Queue`](crate::Queue) can
/// decide the lock granularity.
#[derive(Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct NavigationState {
    #[field(get)]
    current_index: Option<usize>,
    #[field(get, copy, set = set_repeat)]
    repeat_mode: RepeatMode,
    history: VecDeque<usize>,
    #[field(get = is_shuffle_enabled, set = set_shuffle)]
    shuffle_enabled: bool,
    /// Entries [`Self::history`] keeps; the oldest is dropped past it.
    history_limit: usize,
}

impl NavigationState {
    /// New empty state: no current track, history empty, shuffle off,
    /// [`RepeatMode::Off`]. `history_limit` comes from
    /// `QueueSettings::max_history_size`.
    #[must_use]
    pub fn new(history_limit: usize) -> Self {
        Self {
            history_limit,
            current_index: None,
            repeat_mode: RepeatMode::Off,
            history: VecDeque::new(),
            shuffle_enabled: false,
        }
    }

    /// Mark the queue exhausted without selecting a successor.
    pub(crate) fn finish(&mut self) {
        if let Some(current) = self.current_index {
            self.push_history(current);
        }
        self.current_index = None;
    }

    /// Current index, or the last selected index after [`Self::finish`].
    pub(crate) fn last_selected_index(&self) -> Option<usize> {
        self.current_index.or_else(|| self.history.back().copied())
    }

    /// Advance to the next track.
    ///
    /// Returns `None` when the queue is empty or when the end has been
    /// reached with [`RepeatMode::Off`]. With [`RepeatMode::All`] wraps to
    /// index `0`. With [`RepeatMode::One`] returns the current index.
    pub fn next(&mut self, len: usize) -> Option<usize> {
        let current = match (len, self.current_index, self.repeat_mode) {
            (0, _, _) => return None,
            (_, None, _) => {
                self.current_index = Some(0);
                return Some(0);
            }
            (_, Some(current), RepeatMode::One) => return Some(current),
            (_, Some(current), _) => current,
        };
        self.push_history(current);
        let next = match self.repeat_mode {
            _ if current + 1 < len => current + 1,
            RepeatMode::All => 0,
            RepeatMode::Off | RepeatMode::One => {
                self.current_index = None;
                return None;
            }
        };
        self.current_index = Some(next);
        Some(next)
    }

    /// Go back to the previous track. Returns `None` when at index `0` or
    /// when no track has been selected yet.
    pub fn prev(&mut self) -> Option<usize> {
        let current = self.current_index?;
        if current == 0 {
            return None;
        }
        let prev = current - 1;
        self.current_index = Some(prev);
        Some(prev)
    }

    /// Push `track_idx` onto history, deduped against the tail. Past
    /// [`Self::history_limit`] the oldest entry is dropped.
    fn push_history(&mut self, track_idx: usize) {
        if self.history.back() == Some(&track_idx) {
            return;
        }
        if self.history.len() >= self.history_limit {
            self.history.pop_front();
        }
        self.history.push_back(track_idx);
    }

    /// Record an explicit selection. If the previously-current track is
    /// different, it is pushed onto history (deduped against the tail).
    pub fn select(&mut self, idx: usize) {
        if let Some(current) = self.current_index
            && current != idx
            && self.history.back() != Some(&current)
        {
            self.push_history(current);
        }
        self.current_index = Some(idx);
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    /// History deep enough that no test below trips the cap; the cap has
    /// its own test.
    fn nav() -> NavigationState {
        NavigationState::new(16)
    }

    #[kithara::test]
    fn defaults() {
        let nav = nav();
        assert_eq!(nav.current_index(), None);
        assert!(!nav.is_shuffle_enabled());
        assert_eq!(nav.repeat_mode(), RepeatMode::Off);
    }

    /// What the cap buys: past it history forgets its oldest entry instead
    /// of growing for the life of the queue.
    #[kithara::test]
    fn a_full_history_drops_its_oldest_entry() {
        let mut nav = NavigationState::new(2);
        for idx in 0..4 {
            nav.select(idx);
        }
        assert_eq!(nav.history, VecDeque::from(vec![1, 2]));
    }

    #[kithara::test]
    fn select_updates_current_and_pushes_history() {
        let mut nav = nav();
        nav.select(2);
        assert_eq!(nav.current_index(), Some(2));
        assert_eq!(nav.history.len(), 0);
        nav.select(5);
        assert_eq!(nav.current_index(), Some(5));
        assert_eq!(nav.history.back(), Some(&2));
    }

    #[kithara::test]
    fn select_dedupes_adjacent_history() {
        let mut nav = nav();
        nav.select(1);
        nav.select(1);
        nav.select(1);
        assert!(nav.history.is_empty());
    }

    #[kithara::test]
    fn next_from_empty_queue_is_none() {
        let mut nav = nav();
        assert_eq!(nav.next(0), None);
    }

    #[kithara::test]
    fn next_from_unselected_starts_at_zero() {
        let mut nav = nav();
        assert_eq!(nav.next(3), Some(0));
    }

    #[kithara::test]
    fn next_wraps_with_repeat_all() {
        let mut nav = nav();
        nav.set_repeat(RepeatMode::All);
        assert_eq!(nav.next(3), Some(0));
        assert_eq!(nav.next(3), Some(1));
        assert_eq!(nav.next(3), Some(2));
        assert_eq!(nav.next(3), Some(0));
    }

    #[kithara::test]
    fn next_stops_at_end_with_repeat_off() {
        let mut nav = nav();
        nav.select(2);
        assert_eq!(nav.next(3), None);
    }

    #[kithara::test]
    fn finish_clears_current() {
        let mut nav = nav();
        nav.select(2);
        nav.finish();
        assert_eq!(nav.current_index(), None);
    }

    #[kithara::test]
    fn finish_preserves_last_selected_index() {
        let mut nav = nav();
        nav.select(2);
        nav.finish();
        assert_eq!(nav.last_selected_index(), Some(2));
    }

    #[kithara::test]
    fn next_returns_current_with_repeat_one() {
        let mut nav = nav();
        nav.select(1);
        nav.set_repeat(RepeatMode::One);
        assert_eq!(nav.next(3), Some(1));
        assert_eq!(nav.next(3), Some(1));
    }

    #[kithara::test]
    fn prev_at_zero_is_none() {
        let mut nav = nav();
        nav.select(0);
        assert_eq!(nav.prev(), None);
    }

    #[kithara::test]
    fn prev_at_unselected_is_none() {
        let mut nav = nav();
        assert_eq!(nav.prev(), None);
    }

    #[kithara::test]
    fn prev_decrements() {
        let mut nav = nav();
        nav.select(2);
        assert_eq!(nav.prev(), Some(1));
        assert_eq!(nav.prev(), Some(0));
        assert_eq!(nav.prev(), None);
    }

    #[kithara::test]
    fn shuffle_toggle() {
        let mut nav = nav();
        assert!(!nav.is_shuffle_enabled());
        nav.set_shuffle(true);
        assert!(nav.is_shuffle_enabled());
        nav.set_shuffle(false);
        assert!(!nav.is_shuffle_enabled());
    }

    #[kithara::test]
    fn repeat_mode_roundtrip() {
        let mut nav = nav();
        nav.set_repeat(RepeatMode::All);
        assert_eq!(nav.repeat_mode(), RepeatMode::All);
        nav.set_repeat(RepeatMode::One);
        assert_eq!(nav.repeat_mode(), RepeatMode::One);
        nav.set_repeat(RepeatMode::Off);
        assert_eq!(nav.repeat_mode(), RepeatMode::Off);
    }
}
