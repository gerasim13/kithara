#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use kithara_test_utils::kithara;

    use super::*;

    struct TestWake {
        woken: AtomicBool,
    }

    impl WakeSignal for TestWake {
        fn wake(&self) {
            self.woken.store(true, Ordering::SeqCst);
        }
    }

    struct CountingWake {
        count: AtomicUsize,
    }

    impl WakeSignal for CountingWake {
        fn wake(&self) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[kithara::test]
    fn connect_push_pop() {
        let (mut out, mut inl) = connect::<i32>(2, None);
        assert_eq!(inl.try_pop(), None);

        assert_eq!(out.try_push(1), Ok(()));
        assert_eq!(out.try_push(2), Ok(()));
        assert_eq!(out.try_push(3), Ok(()));
        assert_eq!(out.try_push(4), Err(4));

        assert_eq!(inl.try_pop(), Some(1));
        assert_eq!(inl.try_pop(), Some(2));
        assert_eq!(inl.try_pop(), None);

        assert!(out.flush());
        assert_eq!(inl.try_pop(), Some(3));
        assert_eq!(inl.try_pop(), None);
    }

    #[kithara::test]
    fn try_push_drains_overflow_first() {
        let (mut out, mut inl) = connect::<i32>(1, None);

        assert_eq!(out.try_push(1), Ok(()));
        assert_eq!(out.try_push(2), Ok(()));

        assert_eq!(inl.try_pop(), Some(1));
        assert_eq!(out.try_push(3), Ok(()));

        assert_eq!(inl.try_pop(), Some(2));
        assert!(out.flush());
        assert_eq!(inl.try_pop(), Some(3));
    }

    #[kithara::test]
    fn flush_returns_false_when_ring_full() {
        let (mut out, mut inl) = connect::<i32>(1, None);

        assert_eq!(out.try_push(1), Ok(()));
        assert_eq!(out.try_push(2), Ok(()));

        assert!(!out.flush());

        assert_eq!(inl.try_pop(), Some(1));
        assert!(out.flush());
        assert_eq!(inl.try_pop(), Some(2));
        assert_eq!(inl.try_pop(), None);
    }

    #[kithara::test]
    fn direct_push_never_occupies_overflow() {
        let (mut out, mut inl) = connect::<i32>(1, None);

        assert!(out.can_push_direct());
        out.push_direct(1);
        assert!(!out.can_push_direct());
        assert_eq!(inl.try_pop(), Some(1));
        assert_eq!(inl.try_pop(), None);
        assert!(out.can_push_direct());
    }

    #[kithara::test]
    fn wake_signal() {
        let wake = Arc::new(TestWake {
            woken: AtomicBool::new(false),
        });
        let (mut out, _inl) = connect::<i32>(2, Some(wake.clone()));

        assert!(!wake.woken.load(Ordering::SeqCst));
        assert_eq!(out.try_push(42), Ok(()));
        assert!(wake.woken.load(Ordering::SeqCst));
    }

    #[kithara::test]
    fn wake_skipped_when_parking_in_overflow() {
        let wake = Arc::new(CountingWake {
            count: AtomicUsize::new(0),
        });
        let (mut out, mut inl) = connect::<i32>(1, Some(wake.clone()));

        assert_eq!(out.try_push(1), Ok(()));
        assert_eq!(wake.count.load(Ordering::SeqCst), 1);

        // Parks in overflow (ring full): no ring push, so no wake.
        assert_eq!(out.try_push(2), Ok(()));
        assert_eq!(wake.count.load(Ordering::SeqCst), 1);

        assert_eq!(inl.try_pop(), Some(1));
        assert!(out.flush());
        assert_eq!(wake.count.load(Ordering::SeqCst), 2);
    }

    #[kithara::test]
    fn wake_fires_every_ring_push() {
        let wake = Arc::new(CountingWake {
            count: AtomicUsize::new(0),
        });
        let (mut out, mut inl) = connect::<i32>(2, Some(wake.clone()));

        assert_eq!(out.try_push(1), Ok(()));
        assert_eq!(wake.count.load(Ordering::SeqCst), 1);
        assert_eq!(out.try_push(2), Ok(()));
        assert_eq!(wake.count.load(Ordering::SeqCst), 2);
        assert_eq!(inl.try_pop(), Some(1));
        assert_eq!(inl.try_pop(), Some(2));
        assert_eq!(out.try_push(3), Ok(()));
        assert_eq!(wake.count.load(Ordering::SeqCst), 3);
    }
}
