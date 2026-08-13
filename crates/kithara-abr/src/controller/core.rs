use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use bon::Builder;
use dashmap::DashMap;
use kithara_events::{AbrEvent, AbrMode, EventBus};
use kithara_platform::{
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use kithara_test_utils::kithara;

use super::peer::PeerEntry;
use crate::{
    abr::Abr,
    estimator::{Estimator, ThroughputEstimator},
    handle::AbrHandle,
};

struct Defaults;

impl Defaults {
    const BANDWIDTH_EMIT_MIN_INTERVAL: Duration = Duration::from_secs(1);
    const BANDWIDTH_EMIT_MIN_DELTA_RATIO: f64 = 0.10;
    const BUFFER_EMIT_MIN_DELTA: Duration = Duration::from_millis(500);
    const BUFFER_EMIT_MIN_INTERVAL: Duration = Duration::from_millis(500);
    const DOWN_HYSTERESIS_RATIO: f64 = 0.8;
    const INITIAL_THROUGHPUT_BPS: u64 = 2_000_000;
    const MIN_BUFFER_FOR_UP_SWITCH: Duration = Duration::from_secs(10);
    const MIN_SWITCH_INTERVAL: Duration = Duration::from_secs(30);
    const THROUGHPUT_SAFETY_FACTOR: f64 = 1.5;
    const UP_HYSTERESIS_RATIO: f64 = 1.3;
    const URGENT_DOWNSWITCH_BUFFER: Duration = Duration::from_secs(5);
}

/// Opaque peer identifier assigned by the ABR controller on `register`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AbrPeerId(NonZeroU64);

impl AbrPeerId {
    /// Construct from a non-zero identifier.
    #[must_use]
    pub const fn new(id: NonZeroU64) -> Self {
        Self(id)
    }
}

impl kithara_test_utils::probe::IntoProbeArg for AbrPeerId {
    fn into_probe_arg(self) -> u64 {
        self.0.get()
    }
}

/// ABR controller settings.
#[derive(Clone, Debug, PartialEq, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct AbrSettings {
    /// Minimum interval between `AbrEvent::BandwidthEstimate` emits.
    #[builder(default = Defaults::BANDWIDTH_EMIT_MIN_INTERVAL)]
    pub bandwidth_emit_min_interval: Duration,
    /// Minimum absolute delta between `BufferAhead` emits.
    #[builder(default = Defaults::BUFFER_EMIT_MIN_DELTA)]
    pub buffer_emit_min_delta: Duration,
    /// Minimum interval between `AbrEvent::BufferAhead` emits.
    #[builder(default = Defaults::BUFFER_EMIT_MIN_INTERVAL)]
    pub buffer_emit_min_interval: Duration,
    /// Minimum buffer-ahead required before an up-switch is allowed.
    #[builder(default = Defaults::MIN_BUFFER_FOR_UP_SWITCH)]
    pub min_buffer_for_up_switch: Duration,
    /// Minimum interval between variant switches.
    #[builder(default = Defaults::MIN_SWITCH_INTERVAL)]
    pub min_switch_interval: Duration,
    /// Buffer-ahead at or below this threshold forces an urgent down-switch.
    #[builder(default = Defaults::URGENT_DOWNSWITCH_BUFFER)]
    pub urgent_downswitch_buffer: Duration,
    /// Seed throughput estimate (bps) applied at controller construction.
    #[builder(
        required,
        with = Some,
        default = Some(Defaults::INITIAL_THROUGHPUT_BPS)
    )]
    pub initial_throughput_bps: Option<u64>,
    /// Global data-saver cap.
    pub max_bandwidth_bps: Option<u64>,
    /// Minimum relative delta (0.0–1.0) between `BandwidthEstimate` emits.
    #[builder(default = Defaults::BANDWIDTH_EMIT_MIN_DELTA_RATIO)]
    pub bandwidth_emit_min_delta_ratio: f64,
    /// Hysteresis ratio for down-switch.
    #[builder(default = Defaults::DOWN_HYSTERESIS_RATIO)]
    pub down_hysteresis_ratio: f64,
    /// Safety factor applied to the throughput estimate before comparing.
    #[builder(default = Defaults::THROUGHPUT_SAFETY_FACTOR)]
    pub throughput_safety_factor: f64,
    /// Hysteresis ratio for up-switch.
    #[builder(default = Defaults::UP_HYSTERESIS_RATIO)]
    pub up_hysteresis_ratio: f64,
}

impl Default for AbrSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Shared per-player ABR controller.
///
/// Holds the bandwidth estimator (one per controller) and a map of
/// registered peers. Constructed via [`AbrController::new`]; peers are
/// attached with [`AbrController::register`].
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct AbrController {
    #[field(get)]
    pub(super) settings: AbrSettings,
    pub(super) estimator: Arc<dyn Estimator>,
    next_peer_id: AtomicU64,
    peers: DashMap<AbrPeerId, Arc<PeerEntry>>,
}

impl AbrController {
    /// Minimum delay between `AbrEvent::ThroughputSample` emits (fixed).
    pub(super) const MIN_THROUGHPUT_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

    /// Create a new controller with the default [`ThroughputEstimator`].
    ///
    /// `cancel` is the parent token whose subtree gates this controller's
    /// background incoherence watches.
    #[must_use]
    pub fn new(settings: AbrSettings) -> Arc<Self> {
        Self::with_estimator(settings, Arc::new(ThroughputEstimator::new()))
    }

    pub(super) fn allocate_peer_id(&self) -> AbrPeerId {
        let raw = self
            .next_peer_id
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        AbrPeerId::new(
            NonZeroU64::new(raw)
                .unwrap_or_else(|| NonZeroU64::new(1).expect("BUG: 1 is statically non-zero")),
        )
    }

    pub(crate) fn on_locked(&self, peer_id: AbrPeerId) {
        if let Some(entry) = self.peer_entry(peer_id)
            && let Some(bus) = entry.bus()
        {
            bus.publish(AbrEvent::Locked);
        }
    }

    pub(crate) fn on_max_bandwidth_cap_changed(&self, peer_id: AbrPeerId, cap: Option<u64>) {
        if let Some(entry) = self.peer_entry(peer_id)
            && let Some(bus) = entry.bus()
        {
            bus.publish(AbrEvent::MaxBandwidthCapChanged { cap });
        }
        self.tick(peer_id, Instant::now());
    }

    #[kithara::probe(peer_id, mode)]
    pub(crate) fn on_mode_changed(&self, peer_id: AbrPeerId, mode: AbrMode) {
        if let Some(entry) = self.peer_entry(peer_id)
            && let Some(bus) = entry.bus()
        {
            bus.publish(AbrEvent::ModeChanged { mode });
        }
        self.tick(peer_id, Instant::now());
        if let Some(entry) = self.peer_entry(peer_id)
            && let Some(peer) = entry.peer_weak.upgrade()
        {
            // A mode change is work even when the synchronous decision is
            // temporarily locked by a seek. Re-polling the peer synchronizes
            // that lock; its existing unlock edge then re-evaluates the mode.
            peer.wake();
        }
    }

    pub(crate) fn on_unlocked(&self, peer_id: AbrPeerId) {
        if let Some(entry) = self.peer_entry(peer_id)
            && let Some(bus) = entry.bus()
        {
            bus.publish(AbrEvent::Unlocked);
        }
        self.tick(peer_id, Instant::now());
    }

    pub(crate) fn peer_entry(&self, id: AbrPeerId) -> Option<Arc<PeerEntry>> {
        self.peers.get(&id).map(|r| Arc::clone(r.value()))
    }

    /// Register a peer. Returns an [`AbrHandle`] that the caller keeps alive
    /// for the peer's lifetime; the handle's `Drop` unregisters the peer.
    pub fn register(self: &Arc<Self>, peer: &Arc<dyn Abr>) -> AbrHandle {
        let id = self.allocate_peer_id();
        let state = peer.state();
        let peer_weak = Arc::downgrade(peer);
        let bus: Arc<RwLock<Option<EventBus>>> = Arc::new(RwLock::default());
        let entry = Arc::new(PeerEntry {
            peer_weak,
            bus: Arc::clone(&bus),
            variants_registered_published: AtomicBool::new(false),
            bytes_downloaded: AtomicU64::new(0),
            throttle: Mutex::default(),
            state: state.clone(),
        });
        self.peers.insert(id, entry);
        AbrHandle::new(Arc::clone(self), id, state, bus)
    }

    fn seed_estimator(settings: &AbrSettings, estimator: &Arc<dyn Estimator>) {
        if let Some(bps) = settings.initial_throughput_bps {
            estimator.seed_initial_bps(bps);
        }
    }

    /// Called from [`AbrHandle::drop`].
    pub(crate) fn unregister(&self, id: AbrPeerId) {
        self.peers.remove(&id);
    }

    /// Create a new controller with a custom estimator. Used in tests to
    /// inject a mock.
    #[must_use]
    pub fn with_estimator(settings: AbrSettings, estimator: Arc<dyn Estimator>) -> Arc<Self> {
        Self::seed_estimator(&settings, &estimator);
        Arc::new(Self {
            settings,
            estimator,
            next_peer_id: AtomicU64::new(0),
            peers: DashMap::new(),
        })
    }
}
