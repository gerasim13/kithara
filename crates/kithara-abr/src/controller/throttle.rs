use std::sync::atomic::Ordering;

use kithara_events::EventBus;
use kithara_platform::{
    sync::Arc,
    time::{Duration, Instant},
};
use kithara_test_utils::kithara;
use num_traits::ToPrimitive;
use tracing::debug;

use super::{
    core::{AbrController, AbrPeerId},
    peer::PeerEntry,
};
use crate::{AbrEvent, BandwidthSource, state::AbrView};

/// Per-peer throttling state for sample / estimate / buffer events.
#[derive(Default)]
pub(super) struct EventThrottleCache {
    pub(super) last_bandwidth_emit: Option<(Instant, u64)>,
    pub(super) last_buffer_emit: Option<(Instant, Option<Duration>)>,
    pub(super) last_throughput_sample_at: Option<Instant>,
}

impl AbrController {
    /// Record a bandwidth sample for `peer_id`. Called by the Downloader
    /// when a fetch completes. Also evaluates the peer at the sample timestamp,
    /// read on the clock the downloader ticks the controller with.
    #[kithara::flash(true)]
    pub fn record_bandwidth(
        self: &Arc<Self>,
        peer_id: AbrPeerId,
        bytes: u64,
        fetch_duration: Duration,
        source: BandwidthSource,
    ) {
        if fetch_duration.is_zero() {
            debug!(
                ?peer_id,
                bytes, "ABR: bandwidth sample dropped — zero fetch duration"
            );
            return;
        }
        self.estimator.push_sample(bytes, fetch_duration, source);

        let Some(entry) = self.peer_entry(peer_id) else {
            return;
        };

        entry.bytes_downloaded.fetch_add(bytes, Ordering::AcqRel);

        let now = Instant::now();
        let bus = entry.bus();
        if let Some(ref bus) = bus {
            let mut throttle = entry.throttle.lock();
            let emit = throttle.last_throughput_sample_at.is_none_or(|t| {
                now.duration_since(t) >= self.settings.throughput_sample_min_interval
            });
            if emit {
                throttle.last_throughput_sample_at = Some(now);
                drop(throttle);
                let bps = bytes_per_second(bytes, fetch_duration);
                bus.publish(AbrEvent::ThroughputSample {
                    source,
                    bytes_per_second: bps,
                });
            }
        }

        self.run_tick(peer_id, now);
    }

    pub(super) fn emit_throttled(
        &self,
        entry: &PeerEntry,
        bus: &Option<EventBus>,
        now: Instant,
        view: &AbrView<'_>,
    ) {
        let estimate_bps = view.estimate_bps;
        let buffer_ahead = view.buffer_ahead;
        let Some(bus) = bus else {
            return;
        };
        let mut throttle = entry.throttle.lock();

        if let Some(bps) = estimate_bps {
            let should_emit = match throttle.last_bandwidth_emit {
                None => true,
                Some((t, prev)) => {
                    let time_ok =
                        now.duration_since(t) >= self.settings.bandwidth_emit_min_interval;
                    let delta_ok =
                        relative_delta(prev, bps) >= self.settings.bandwidth_emit_min_delta_ratio;
                    time_ok || delta_ok
                }
            };
            if should_emit {
                throttle.last_bandwidth_emit = Some((now, bps));
                bus.publish(AbrEvent::BandwidthEstimate { bps });
            }
        }

        let should_emit_buffer = match throttle.last_buffer_emit {
            None => true,
            Some((t, prev)) => {
                let time_ok = now.duration_since(t) >= self.settings.buffer_emit_min_interval;
                let transition = prev.is_some() != buffer_ahead.is_some();
                let delta_ok = match (prev, buffer_ahead) {
                    (Some(a), Some(b)) => a.abs_diff(b) >= self.settings.buffer_emit_min_delta,
                    _ => true,
                };
                transition || (time_ok && delta_ok)
            }
        };
        if should_emit_buffer {
            throttle.last_buffer_emit = Some((now, buffer_ahead));
            drop(throttle);
            bus.publish(AbrEvent::BufferAhead {
                ahead: buffer_ahead,
            });
        }
    }
}

pub(super) fn bytes_per_second(bytes: u64, duration: Duration) -> f64 {
    let secs = duration.as_secs_f64().max(f64::EPSILON);
    let bytes_f = bytes.to_f64().unwrap_or(0.0);
    bytes_f / secs
}

fn relative_delta(prev: u64, now: u64) -> f64 {
    if prev == 0 {
        return f64::INFINITY;
    }
    let diff = (i128::from(prev) - i128::from(now))
        .unsigned_abs()
        .to_f64()
        .unwrap_or(f64::INFINITY);
    let base = prev.to_f64().unwrap_or(f64::INFINITY);
    diff / base
}
