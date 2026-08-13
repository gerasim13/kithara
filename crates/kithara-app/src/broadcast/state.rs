use std::{error::Error, mem};

use kithara::play::SessionHandle;
use kithara_platform::{
    CancelToken,
    time::{Duration, Instant},
    tokio::task,
};

pub(crate) type BroadcastResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// What the studio bar needs from a packager.
///
/// `Live` is a running stream. Where the `broadcast` feature is off it has no
/// values at all, which makes [`Phase::Running`] a variant nothing can build:
/// the on-air half of this state machine is then unreachable by construction
/// rather than by a branch, and the compiler drops it.
pub(crate) trait Packager: 'static {
    type Live: Send + 'static;

    /// `Ok(None)` means the session has not measured a device rate yet — the
    /// request stands and the next frame asks again.
    fn start(
        session: &SessionHandle,
        shutdown: &CancelToken,
    ) -> BroadcastResult<Option<Self::Live>>;

    fn is_live(live: &Self::Live) -> bool;

    fn url(live: &Self::Live) -> &str;

    /// Drains the stream and shuts it down. Blocking.
    fn stop(live: Self::Live);
}

pub(crate) struct Broadcaster<P: Packager> {
    session: SessionHandle,
    shutdown: CancelToken,
    phase: Phase<P>,
}

enum Phase<P: Packager> {
    Off,
    Requested,
    Running { live: P::Live },
    Stopping,
}

/// A stream handed over for shutdown. The bar is already `Stopping` by then, so
/// the drain runs off the frame loop and reports back through a message.
pub(crate) struct BroadcastStop<P: Packager>(P::Live);

impl<P: Packager> Broadcaster<P> {
    pub(crate) const fn new(session: SessionHandle, shutdown: CancelToken) -> Self {
        Self {
            session,
            shutdown,
            phase: Phase::Off,
        }
    }

    pub(crate) fn poll(&mut self) {
        if matches!(&self.phase, Phase::Running { live } if !P::is_live(live)) {
            self.phase = Phase::Off;
            return;
        }
        if !matches!(self.phase, Phase::Requested) {
            return;
        }
        match P::start(&self.session, &self.shutdown) {
            Ok(Some(live)) => {
                tracing::info!(url = P::url(&live), "broadcast is live");
                self.phase = Phase::Running { live };
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "broadcast did not start");
                self.phase = Phase::Off;
            }
        }
    }

    pub(crate) fn toggle(&mut self) -> Option<BroadcastStop<P>> {
        match mem::replace(&mut self.phase, Phase::Off) {
            Phase::Off => self.phase = Phase::Requested,
            Phase::Requested => {}
            Phase::Running { live } => {
                self.phase = Phase::Stopping;
                return Some(BroadcastStop(live));
            }
            Phase::Stopping => self.phase = Phase::Stopping,
        }
        None
    }

    pub(crate) fn complete_stop(&mut self) {
        if matches!(self.phase, Phase::Stopping) {
            self.phase = Phase::Off;
        }
    }

    /// The stream is serving. A request waiting for the device rate is not yet
    /// on air, and neither is a stop that has not finished draining.
    pub(crate) const fn is_on_air(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    pub(crate) fn url(&self) -> Option<&str> {
        match &self.phase {
            Phase::Running { live } => Some(P::url(live)),
            Phase::Off | Phase::Requested | Phase::Stopping => None,
        }
    }
}

impl<P: Packager> BroadcastStop<P> {
    pub(crate) async fn run(self) -> Option<Duration> {
        let drain = task::spawn_blocking(move || {
            let started = Instant::now();
            P::stop(self.0);
            started.elapsed()
        });
        match drain.await {
            Ok(duration) => Some(duration),
            Err(error) => {
                tracing::error!(%error, "broadcast stop worker failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara::{
        audio::ConsumerWakeMode,
        play::{Cmd, PlayError, Reply, SessionDispatcher},
    };
    use kithara_platform::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    /// The phases carry no session work — only a packager reaches the session —
    /// so a dispatcher that answers nothing is the honest stand-in, and a
    /// machine that did reach it fails here instead of silently passing.
    struct NoSession;

    impl SessionDispatcher for NoSession {
        fn exec(&self, _cmd: Cmd) -> Result<Reply, PlayError> {
            panic!("the state machine must not reach the session")
        }

        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }
    }

    fn broadcaster<P: Packager>() -> Broadcaster<P> {
        Broadcaster::new(SessionHandle::new(Arc::new(NoSession)), CancelToken::root())
    }

    /// Owned and not `Copy`, the way a real stream handle is.
    struct Stream(String);

    /// Liveness of [`Ready`]'s stream. `start` raises it, so a test that lowers
    /// it does not leak into one that runs after.
    static LIVE: AtomicBool = AtomicBool::new(true);

    /// Goes live on the first poll and stays live until [`LIVE`] is lowered.
    struct Ready;

    impl Ready {
        const URL: &str = "http://packager.test/master.m3u8";

        /// The one place this fake builds its stream, so what the packager
        /// answers and what a test asserts cannot drift apart.
        fn stream() -> Stream {
            Stream(Self::URL.to_owned())
        }
    }

    impl Packager for Ready {
        type Live = Stream;

        fn start(
            _session: &SessionHandle,
            _shutdown: &CancelToken,
        ) -> BroadcastResult<Option<Stream>> {
            LIVE.store(true, Ordering::Relaxed);
            Ok(Some(Self::stream()))
        }

        fn is_live(_live: &Stream) -> bool {
            LIVE.load(Ordering::Relaxed)
        }

        fn url(live: &Stream) -> &str {
            &live.0
        }

        fn stop(_live: Stream) {}
    }

    /// A session that has not reported a device rate yet.
    struct Unmeasured;

    impl Packager for Unmeasured {
        type Live = Stream;

        fn start(
            _session: &SessionHandle,
            _shutdown: &CancelToken,
        ) -> BroadcastResult<Option<Stream>> {
            Ok(None)
        }

        fn is_live(_live: &Stream) -> bool {
            true
        }

        fn url(live: &Stream) -> &str {
            &live.0
        }

        fn stop(_live: Stream) {}
    }

    /// A packager that refuses — the shape a build without the feature has.
    struct Absent;

    impl Packager for Absent {
        type Live = Stream;

        fn start(
            _session: &SessionHandle,
            _shutdown: &CancelToken,
        ) -> BroadcastResult<Option<Stream>> {
            Err("no packager in this build".into())
        }

        fn is_live(_live: &Stream) -> bool {
            true
        }

        fn url(live: &Stream) -> &str {
            &live.0
        }

        fn stop(_live: Stream) {}
    }

    #[test]
    fn a_request_without_a_measured_rate_keeps_asking() {
        let mut bar = broadcaster::<Unmeasured>();

        bar.toggle();
        bar.poll();
        bar.poll();

        assert!(matches!(bar.phase, Phase::Requested));
    }

    #[test]
    fn a_request_no_packager_can_serve_returns_the_bar_to_off() {
        let mut bar = broadcaster::<Absent>();

        bar.toggle();
        bar.poll();

        assert!(matches!(bar.phase, Phase::Off));
        assert!(!bar.is_on_air());
    }

    #[test]
    fn a_served_request_puts_the_bar_on_air_with_the_stream_url() {
        let mut bar = broadcaster::<Ready>();

        bar.toggle();
        assert!(!bar.is_on_air(), "a request is not yet a stream");
        bar.poll();

        assert!(bar.is_on_air());
        assert_eq!(bar.url(), Some(Ready::URL));
    }

    #[test]
    fn stopping_hands_over_a_job_and_finishes_only_on_completion() {
        let mut bar = broadcaster::<Ready>();
        bar.toggle();
        bar.poll();

        let stop = bar.toggle().expect("a running stream hands over its stop");
        assert!(matches!(bar.phase, Phase::Stopping));
        assert!(!bar.is_on_air());

        Ready::stop(stop.0);
        assert!(
            matches!(bar.phase, Phase::Stopping),
            "the bar waits for the drain to report back"
        );

        bar.complete_stop();
        assert!(matches!(bar.phase, Phase::Off));
    }

    #[test]
    fn a_stream_that_ends_on_its_own_is_noticed_by_the_next_poll() {
        let mut bar = broadcaster::<Ready>();
        bar.toggle();
        bar.poll();
        assert!(bar.is_on_air());

        LIVE.store(false, Ordering::Relaxed);
        bar.poll();

        assert!(matches!(bar.phase, Phase::Off));
    }

    /// Pressing REC again while the request is still waiting for a device rate
    /// withdraws it. There is no stream yet, so there is no stop job either —
    /// the bar simply goes back to off and stops asking.
    #[test]
    fn toggling_a_pending_request_withdraws_it() {
        let mut bar = broadcaster::<Unmeasured>();
        bar.toggle();

        assert!(bar.toggle().is_none(), "there is no stream to stop yet");
        assert!(matches!(bar.phase, Phase::Off));

        bar.poll();
        assert!(
            matches!(bar.phase, Phase::Off),
            "a withdrawn request must not start on the next frame"
        );
    }
}
