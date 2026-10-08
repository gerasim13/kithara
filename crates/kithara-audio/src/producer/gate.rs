#[cfg(test)]
mod tests {
    use kithara_platform::{
        sync::Arc,
        thread,
        time::{Duration, timeout},
    };
    use kithara_test_utils::kithara;

    use super::PreloadGate;

    #[kithara::test(tokio)]
    async fn wait_resolves_after_signal() {
        let gate = Arc::new(PreloadGate::default());
        assert!(!gate.is_ready());

        let signaller = Arc::clone(&gate);
        // spawn_named: the signaller must be engine-visible under flash — a bare
        // spawn sleeps in REAL time while the waiter's poll loop and the timeout
        // run on the virtual clock, so the virtual 1s can elapse before the real
        // 5ms signal lands (mixed-clock race).
        let join = thread::spawn_named("preload-signal", move || {
            thread::sleep(Duration::from_millis(5));
            signaller.signal_epoch(0);
        });

        timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect("signal must open the gate");
        assert!(gate.is_ready());
        join.join().expect("signaller thread");
    }

    #[kithara::test(tokio)]
    async fn rearm_reblocks_a_fresh_wait() {
        let gate = Arc::new(PreloadGate::default());
        gate.signal_epoch(0);
        gate.wait().await;

        gate.rearm();
        assert!(!gate.is_ready());

        let re_signaller = Arc::clone(&gate);
        // spawn_named for the same mixed-clock reason as wait_resolves_after_signal.
        let join = thread::spawn_named("preload-resignal", move || {
            thread::sleep(Duration::from_millis(5));
            re_signaller.signal_epoch(0);
        });

        timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect("re-armed gate must reopen on the next signal");
        join.join().expect("re-signaller thread");
    }

    #[kithara::test]
    fn old_epoch_signal_does_not_open_new_epoch_wait() {
        let gate = PreloadGate::default();

        gate.signal_epoch(0);
        assert!(gate.is_ready_for_epoch(0));

        gate.rearm();
        gate.signal_epoch(0);

        assert!(gate.is_ready());
        assert!(!gate.is_ready_for_epoch(1));
    }
}
