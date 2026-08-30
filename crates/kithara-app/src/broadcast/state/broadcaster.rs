use std::mem;

use kithara::host::Host;
use kithara_platform::{
    CancelToken,
    time::{Duration, Instant},
    tokio::task,
};

use super::Packager;

pub(crate) struct Broadcaster<P: Packager> {
    pub(super) phase: Phase<P>,
    shutdown: CancelToken,
    tap_lead: Duration,
}

pub(super) enum Phase<P: Packager> {
    Off,
    Requested,
    Running { live: P::Live },
    Stopping,
}

/// A stream handed over for shutdown; the drain runs off the frame loop.
pub(crate) struct BroadcastStop<P: Packager>(pub(super) P::Live);

impl<P: Packager> Broadcaster<P> {
    pub(crate) const fn new(shutdown: CancelToken, tap_lead: Duration) -> Self {
        Self {
            shutdown,
            tap_lead,
            phase: Phase::Off,
        }
    }

    pub(crate) fn complete_stop(&mut self) {
        if matches!(self.phase, Phase::Stopping) {
            self.phase = Phase::Off;
        }
    }

    pub(crate) const fn is_available() -> bool {
        P::IS_AVAILABLE
    }

    /// Serving. A pending request and a draining stop are both off air.
    pub(crate) const fn is_on_air(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    pub(crate) fn poll(&mut self, host: &Host) {
        if matches!(&self.phase, Phase::Running { live } if !P::is_live(live)) {
            if let Err(error) = P::release(host) {
                tracing::error!(%error, "failed to release stopped broadcast mix tap");
            }
            self.phase = Phase::Off;
            return;
        }
        if !matches!(self.phase, Phase::Requested) {
            return;
        }
        match P::start(host, &self.shutdown, self.tap_lead) {
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

    pub(crate) fn release(&mut self, host: &Host) {
        if matches!(self.phase, Phase::Running { .. })
            && let Err(error) = P::release(host)
        {
            tracing::error!(%error, "failed to release broadcast mix tap during shutdown");
        }
    }

    pub(crate) fn toggle(&mut self, host: &Host) -> Option<BroadcastStop<P>> {
        match mem::replace(&mut self.phase, Phase::Off) {
            Phase::Off => self.phase = Phase::Requested,
            Phase::Requested => {}
            Phase::Running { live } => {
                if let Err(error) = P::release(host) {
                    tracing::error!(%error, "failed to release broadcast mix tap");
                }
                self.phase = Phase::Stopping;
                return Some(BroadcastStop(live));
            }
            Phase::Stopping => self.phase = Phase::Stopping,
        }
        None
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
