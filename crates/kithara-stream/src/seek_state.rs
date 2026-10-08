
#[cfg(test)]
mod tests {
    use std::{sync::Barrier, thread};

    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;

    fn state() -> SeekState {
        SeekState::new()
    }

    fn flags_snapshot(s: &SeekState, order: Ordering) -> TimelineFlags {
        TimelineFlags::from_bits_truncate(s.flags_raw(order))
    }

    /// `begin` returns strictly increasing epochs.
    #[kithara::test]
    fn epoch_monotonicity() {
        let s = state();
        let e1 = s.begin(Duration::from_secs(1));
        let e2 = s.begin(Duration::from_secs(2));
        let e3 = s.begin(Duration::from_secs(3));
        assert_eq!(e1, 1);
        assert_eq!(e2, 2);
        assert_eq!(e3, 3);
        assert_eq!(s.epoch(), 3);
    }

    #[kithara::test]
    fn commit_if_epoch_runs_only_for_the_current_epoch() {
        let s = state();
        assert_eq!(s.commit_if_epoch(0, || 7), Some(7));

        let current = s.begin(Duration::from_secs(1));

        assert_eq!(s.commit_if_epoch(0, || 9), None);
        assert_eq!(s.commit_if_epoch(current, || 11), Some(11));
    }

    #[kithara::test]
    fn seek_epoch_arc_observes_begun_seeks() {
        let s = state();
        let epoch = s.seek_epoch_arc();
        assert_eq!(epoch.load(Ordering::Acquire), 0);

        assert_eq!(s.begin(Duration::from_secs(1)), 1);
        assert_eq!(epoch.load(Ordering::Acquire), 1);
    }

    #[kithara::test]
    fn pending_epoch_marks_and_clears_only_the_matching_seek() {
        let s = state();
        assert_eq!(s.pending_epoch(), None);
        assert!(!s.clear_pending_epoch(1));

        let stale = s.begin(Duration::from_secs(1));
        let current = s.begin(Duration::from_secs(2));
        s.mark_pending(current);
        assert_eq!(s.pending_epoch(), Some(current));
        assert!(!s.clear_pending_epoch(stale));
        assert_eq!(s.pending_epoch(), Some(current));
        assert!(s.clear_pending_epoch(current));
        assert_eq!(s.pending_epoch(), None);
        assert!(!s.clear_pending_epoch(current));
    }

    #[kithara::test]
    fn decoder_seek_latch_is_one_shot_per_begin() {
        let s = state();
        assert!(!s.take_decoder_seek());

        s.begin(Duration::from_secs(1));
        assert!(s.take_decoder_seek());
        assert!(!s.take_decoder_seek());
    }

    /// `complete(old_epoch)` must NOT clear a newer seek's flushing flag.
    #[kithara::test]
    fn stale_complete_leaves_newer_seek_intact() {
        let s = state();
        let e1 = s.begin(Duration::from_secs(5));
        let _e2 = s.begin(Duration::from_secs(10));
        s.complete(e1);
        assert!(
            s.is_flushing(),
            "newer seek's FLUSHING must survive a stale complete"
        );
        assert_eq!(s.target(), Some(Duration::from_secs(10)));
    }

    /// `take_preempt` returns `true` exactly once after `begin`, then `false`.
    #[kithara::test]
    fn latch_one_shot() {
        let s = state();
        s.begin(Duration::from_secs(3));
        assert!(s.take_preempt(), "first take must be true");
        assert!(!s.take_preempt(), "second take must be false");
        assert!(!s.take_preempt(), "subsequent takes must be false");
    }

    #[kithara::test]
    fn initiate_seek_sets_flushing_and_target() {
        let s = state();
        assert!(!s.is_flushing());
        assert!(s.target().is_none());

        let epoch = s.begin(Duration::from_secs(10));
        assert_eq!(epoch, 1);
        assert!(s.is_flushing());
        assert_eq!(s.target(), Some(Duration::from_secs(10)));
        assert_eq!(s.epoch(), 1);
    }

    #[kithara::test]
    fn complete_seek_clears_flushing() {
        let s = state();
        let epoch = s.begin(Duration::from_secs(5));
        s.complete(epoch);
        assert!(!s.is_flushing());
        assert_eq!(s.target(), Some(Duration::from_secs(5)));
    }

    #[kithara::test]
    fn complete_seek_ignores_stale_epoch() {
        let s = state();
        let epoch1 = s.begin(Duration::from_secs(5));
        let epoch2 = s.begin(Duration::from_secs(10));
        s.complete(epoch1);
        assert!(s.is_flushing());
        assert_eq!(s.target(), Some(Duration::from_secs(10)));
        s.complete(epoch2);
        assert!(!s.is_flushing());
    }

    #[kithara::test]
    fn seek_epoch_monotonically_increases() {
        let s = state();
        let e1 = s.begin(Duration::from_secs(1));
        let e2 = s.begin(Duration::from_secs(2));
        let e3 = s.begin(Duration::from_secs(3));
        assert_eq!(e1, 1);
        assert_eq!(e2, 2);
        assert_eq!(e3, 3);
        assert_eq!(s.target(), Some(Duration::from_secs(3)));
    }

    #[kithara::test]
    fn complete_seek_does_not_clobber_concurrent_target() {
        let s = state();
        let epoch1 = s.begin(Duration::from_secs(5));
        let _epoch2 = s.begin(Duration::from_secs(15));
        s.complete(epoch1);
        assert!(s.is_flushing());
        assert_eq!(s.target(), Some(Duration::from_secs(15)));
    }

    #[kithara::test]
    fn initiate_seek_is_visible_across_arc_clones() {
        let s = Arc::new(state());
        let clone = Arc::clone(&s);
        let _ = s.begin(Duration::from_secs(7));
        assert!(clone.is_flushing());
        assert_eq!(clone.target(), Some(Duration::from_secs(7)));
    }

    #[kithara::test]
    fn initiate_seek_sets_seek_pending() {
        let s = state();
        assert!(!s.is_pending());
        let _epoch = s.begin(Duration::from_secs(5));
        assert!(s.is_pending());
    }

    #[kithara::test]
    fn clear_seek_pending_only_clears_matching_epoch() {
        let s = state();
        let epoch1 = s.begin(Duration::from_secs(5));
        let epoch2 = s.begin(Duration::from_secs(10));
        s.clear_pending(epoch1);
        assert!(s.is_pending());
        s.clear_pending(epoch2);
        assert!(!s.is_pending());
    }

    #[kithara::test]
    fn new_initiate_seek_resets_seek_pending() {
        let s = state();
        let epoch = s.begin(Duration::from_secs(5));
        s.clear_pending(epoch);
        assert!(!s.is_pending());
        let _epoch2 = s.begin(Duration::from_secs(10));
        assert!(s.is_pending());
    }

    #[kithara::test]
    fn complete_seek_does_not_clear_seek_pending() {
        let s = state();
        let epoch = s.begin(Duration::from_secs(5));
        s.complete(epoch);
        assert!(!s.is_flushing());
        assert!(s.is_pending());
    }

    #[kithara::test]
    fn is_seek_pending_visible_across_arc_clones() {
        let s = Arc::new(state());
        let clone = Arc::clone(&s);
        let _epoch = s.begin(Duration::from_secs(5));
        assert!(clone.is_pending());
    }

    #[kithara::test]
    fn flag_pair_matrix_matches_bitflags_snapshot() {
        for mask in 0u8..4 {
            let s = state();
            let want_flushing = mask & 1 != 0;
            let want_seek_pending = mask & 2 != 0;

            if want_flushing || want_seek_pending {
                let _ = s.begin(Duration::from_secs(1));
                if !want_flushing {
                    s.complete(s.epoch());
                }
                if !want_seek_pending {
                    s.clear_pending(s.epoch());
                }
            }

            assert_eq!(s.is_flushing(), want_flushing, "mask {mask:#04b} flushing");
            assert_eq!(
                s.is_pending(),
                want_seek_pending,
                "mask {mask:#04b} seek_pending"
            );

            let snapshot = flags_snapshot(&s, Ordering::Acquire);
            assert_eq!(
                snapshot.contains(TimelineFlags::FLUSHING),
                want_flushing,
                "mask {mask:#04b} snapshot flushing"
            );
            assert_eq!(
                snapshot.contains(TimelineFlags::SEEK_PENDING),
                want_seek_pending,
                "mask {mask:#04b} snapshot seek_pending"
            );
        }
    }

    #[kithara::test]
    fn complete_seek_double_check_re_raises_flushing_when_newer_seek_interleaves() {
        let s = state();
        let epoch1 = s.begin(Duration::from_secs(1));

        s.remove_flags_raw(TimelineFlags::FLUSHING, Ordering::SeqCst);
        let _epoch2 = s.begin(Duration::from_secs(2));
        s.complete(epoch1);

        assert!(
            s.is_flushing(),
            "FLUSHING must be re-raised when a newer seek interleaves mid-complete"
        );
    }

    #[kithara::test]
    fn concurrent_flag_toggles_preserve_independent_semantics() {
        const ITER: usize = 50_000;

        let s = Arc::new(state());
        let barrier = Arc::new(Barrier::new(3));

        let s_a = Arc::clone(&s);
        let barrier_a = Arc::clone(&barrier);
        let a = thread::spawn(move || {
            barrier_a.wait();
            for i in 0..ITER {
                s_a.set_playing(i % 2 == 0);
            }
        });

        let s_b = Arc::clone(&s);
        let barrier_b = Arc::clone(&barrier);
        let b = thread::spawn(move || {
            barrier_b.wait();
            for _ in 0..ITER {
                let epoch = s_b.begin(Duration::from_millis(1));
                s_b.clear_pending(epoch);
                s_b.complete(epoch);
            }
        });

        let s_c = Arc::clone(&s);
        let barrier_c = Arc::clone(&barrier);
        let c = thread::spawn(move || {
            barrier_c.wait();
            let mut observed = 0u64;
            for _ in 0..ITER {
                let snap = flags_snapshot(&s_c, Ordering::Acquire);
                observed ^= u64::from(snap.bits());
            }
            observed
        });

        a.join()
            .expect("BUG: spawned thread A must not panic in this test");
        b.join()
            .expect("BUG: spawned thread B must not panic in this test");
        let _ = c
            .join()
            .expect("BUG: spawned thread C must not panic in this test");

        assert!(
            !s.is_playing(),
            "PLAYING must match the last deterministic write"
        );
        assert!(!s.is_flushing(), "FLUSHING must be fully cleared");
        assert!(
            !s.is_pending(),
            "SEEK_PENDING must be fully cleared after last clear"
        );
    }

    #[kithara::test]
    fn playing_defaults_to_false() {
        let s = state();
        assert!(!s.is_playing());
    }

    #[kithara::test]
    fn set_playing_true_is_visible_across_arc_clones() {
        let s = Arc::new(state());
        let clone = Arc::clone(&s);
        s.set_playing(true);
        assert!(clone.is_playing());
        clone.set_playing(false);
        assert!(!s.is_playing());
    }

    #[kithara::test]
    fn set_playing_idempotent() {
        let s = state();
        s.set_playing(true);
        s.set_playing(true);
        assert!(s.is_playing());
        s.set_playing(false);
        s.set_playing(false);
        assert!(!s.is_playing());
    }

    #[kithara::test]
    fn playing_is_orthogonal_to_other_flags() {
        for mask in 0u8..4 {
            for &initial_playing in &[false, true] {
                let s = state();
                let want_flushing = mask & 1 != 0;
                let want_seek_pending = mask & 2 != 0;

                if want_flushing || want_seek_pending {
                    let _ = s.begin(Duration::from_secs(1));
                    if !want_flushing {
                        s.complete(s.epoch());
                    }
                    if !want_seek_pending {
                        s.clear_pending(s.epoch());
                    }
                }
                s.set_playing(initial_playing);

                assert_eq!(s.is_playing(), initial_playing);
                assert_eq!(
                    s.is_flushing(),
                    want_flushing,
                    "mask {mask:#04b} play={initial_playing} flushing"
                );
                assert_eq!(
                    s.is_pending(),
                    want_seek_pending,
                    "mask {mask:#04b} play={initial_playing} seek_pending"
                );

                s.set_playing(!initial_playing);
                assert_eq!(s.is_playing(), !initial_playing);
                assert_eq!(s.is_flushing(), want_flushing);
                assert_eq!(s.is_pending(), want_seek_pending);
            }
        }
    }

    #[kithara::test]
    fn initiate_seek_does_not_touch_playing() {
        let s = state();
        s.set_playing(true);
        let epoch = s.begin(Duration::from_secs(5));
        assert!(s.is_playing(), "PLAYING must not be affected by seek");
        s.complete(epoch);
        assert!(s.is_playing(), "PLAYING must survive complete_seek");
        s.clear_pending(epoch);
        assert!(s.is_playing(), "PLAYING must survive clear_seek_pending");
    }
}
