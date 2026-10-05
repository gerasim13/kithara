use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use bitflags::bitflags;
use kithara_platform::{
    sync::Mutex,
    time::{Duration, Instant},
};
use num_traits::ToPrimitive;

use super::{decision::AbrDecision, pending::PendingState, view::AbrView};
use crate::{AbrMode, VariantIndex};

bitflags! {
    /// Composable boolean control-state for [`AbrState`], orthogonal to the
    /// `mode` (Auto/Manual, carries an index) and the reentrant `lock` count.
    /// Lives in its own [`AtomicU8`] — room to grow as more escape-class
    /// states are added.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct AbrFlags: u8 {
        /// The active variant cannot deliver the segment the reader is blocked
        /// on (set by the HLS stall detector). While set, `evaluate()` excludes
        /// the current variant from candidacy and skips the buffer-too-low
        /// up-switch gate — staying cannot grow the buffer.
        const ESCAPE = 1 << 0;
    }
}

/// Per-peer ABR state owned by a peer and shared with the controller.
///
/// Variants live in the peer (single source of truth) and reach the
/// state's `decide()` via [`AbrView::variants`]. The state itself only
/// tracks runtime control: current index, mode, switch timing, locks,
/// pending boundary commit.
pub struct AbrState {
    pub(super) current_variant: AtomicUsize,
    last_switch_at_nanos: AtomicU64,
    max_bandwidth_bps: AtomicU64,
    flags: AtomicU8,
    lock_count: AtomicUsize,
    pub(super) mode: AtomicUsize,
    reference_instant: Instant,
    /// Phase 2 boundary-commit slot. `Some` means a switch has been
    /// requested via [`request_target`](AbrState::request_target) and
    /// the scheduler has not yet observed it on a segment boundary.
    /// Replace-pending semantics: a fresh `request_target` overwrites
    /// any prior unobserved entry (latest-wins, matching the
    /// "switch-only-on-boundaries" contract from the two-cursor plan).
    pub(super) pending: Mutex<PendingState>,
}

impl AbrState {
    const NO_BANDWIDTH_CAP: u64 = 0;
    const NO_SWITCH: u64 = 0;

    /// Build an `AbrState` with the initial variant set from `mode`.
    #[must_use]
    pub fn new(mode: AbrMode) -> Self {
        Self::new_at(mode, Instant::now())
    }

    /// Clear the escape condition — the active variant is delivering again, or
    /// a switch off it has been published. Idempotent.
    pub fn clear_escape(&self) {
        self.flags
            .fetch_and(!AbrFlags::ESCAPE.bits(), Ordering::AcqRel);
    }

    #[must_use]
    pub fn current_variant_index(&self) -> VariantIndex {
        VariantIndex::new(self.current_variant.load(Ordering::Acquire))
    }

    /// Produce a decision without mutating state.
    #[must_use]
    pub fn decide(&self, view: &AbrView<'_>, now: Instant) -> AbrDecision {
        super::decision::evaluate(self, view, now)
    }

    fn instant_to_nanos(&self, instant: Instant) -> u64 {
        let nanos = instant
            .saturating_duration_since(self.reference_instant)
            .as_nanos()
            .to_u64()
            .unwrap_or(u64::MAX);
        nanos.max(1)
    }

    /// `true` while the active variant is flagged non-delivering. Read by
    /// `evaluate()` to exclude the variant and skip the buffer up-switch gate.
    #[must_use]
    pub fn is_escaping(&self) -> bool {
        AbrFlags::from_bits_truncate(self.flags.load(Ordering::Acquire)).contains(AbrFlags::ESCAPE)
    }

    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.lock_count.load(Ordering::Acquire) > 0
    }

    /// Gate publication of pending decisions: while `lock_count > 0`,
    /// [`peek_pending_decision`](Self::peek_pending_decision) returns
    /// `None` so the boundary commit defers until unlock. The pending
    /// intent itself is preserved across lock/unlock.
    pub fn lock(&self) {
        let state = self.pending.lock();
        self.lock_count.fetch_add(1, Ordering::AcqRel);
        drop(state);
    }

    #[must_use]
    pub fn lock_count(&self) -> usize {
        self.lock_count.load(Ordering::Acquire)
    }

    /// Flag the active variant as non-delivering — set by the HLS stall
    /// detector when the reader is parked at a clean boundary on a segment
    /// whose in-flight fetch has crossed the downloader's `soft_timeout`.
    /// Idempotent. The caller triggers an out-of-band re-tick (see
    /// [`AbrHandle::reevaluate`](crate::AbrHandle::reevaluate)) so `evaluate()`
    /// observes it.
    pub fn mark_escape(&self) {
        self.flags
            .fetch_or(AbrFlags::ESCAPE.bits(), Ordering::AcqRel);
    }

    #[must_use]
    pub fn max_bandwidth_bps(&self) -> Option<u64> {
        let v = self.max_bandwidth_bps.load(Ordering::Acquire);
        if v == Self::NO_BANDWIDTH_CAP {
            None
        } else {
            Some(v)
        }
    }

    #[must_use]
    pub fn mode(&self) -> AbrMode {
        AbrMode::from(self.mode.load(Ordering::Acquire))
    }

    /// Build an `AbrState` whose session starts at `reference`.
    ///
    /// The anti-oscillation interval before the first switch is measured from
    /// the session's start, so a test that states what that interval holds has
    /// to be able to say when the session started - otherwise the assertion is
    /// a race against its own setup.
    #[must_use]
    pub(crate) fn new_at(mode: AbrMode, reference: Instant) -> Self {
        let initial_variant = match mode {
            AbrMode::Auto(Some(idx)) | AbrMode::Manual(idx) => idx.get(),
            AbrMode::Auto(None) => 0,
        };
        Self {
            current_variant: AtomicUsize::new(initial_variant),
            last_switch_at_nanos: AtomicU64::new(Self::NO_SWITCH),
            max_bandwidth_bps: AtomicU64::new(Self::NO_BANDWIDTH_CAP),
            lock_count: AtomicUsize::new(0),
            mode: AtomicUsize::new(mode.into()),
            flags: AtomicU8::new(AbrFlags::empty().bits()),
            reference_instant: reference,
            pending: Mutex::default(),
        }
    }

    pub(super) fn record_switch(&self, now: Instant) {
        self.last_switch_at_nanos
            .store(self.instant_to_nanos(now), Ordering::Release);
    }

    pub fn set_max_bandwidth_bps(&self, cap: Option<u64>) {
        self.max_bandwidth_bps
            .store(cap.unwrap_or(Self::NO_BANDWIDTH_CAP), Ordering::Release);
    }

    pub(crate) fn switch_interval_remaining(
        &self,
        now: Instant,
        min_interval: Duration,
    ) -> Duration {
        let nanos = self.last_switch_at_nanos.load(Ordering::Acquire);
        let since = if nanos == Self::NO_SWITCH {
            self.reference_instant
        } else {
            self.reference_instant + Duration::from_nanos(nanos)
        };
        min_interval.saturating_sub(now.duration_since(since))
    }

    pub fn unlock(&self) {
        let prev = self.lock_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "unlock called without matching lock");
    }

    delegate::delegate! {
        to self {
            /// Whether the anti-oscillation interval has elapsed. Before the first
            /// switch it is measured from the start of the session: that is when the
            /// throughput estimate rests on the fewest samples, so a fast first segment
            /// must not be enough to flip the variant out from under a listener who has
            /// barely started playing.
            #[expr($.is_zero())]
            #[call(switch_interval_remaining)]
            pub(super) fn can_switch_now(&self, now: Instant, min_interval: Duration) -> bool;
        }
    }
}
