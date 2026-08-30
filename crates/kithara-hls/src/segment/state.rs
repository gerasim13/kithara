use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use bitflags::bitflags;
use kithara_platform::sync::Arc;

use crate::{
    segment::fetch::{FetchClaim, PlannedFetch},
    signal::SizeSignal,
    variant::{HlsVariant, PlanRevision},
};

bitflags! {
    /// Lock-free slot flags packed into one `AtomicU8`. The cache state is
    /// mutually exclusive — at most one of [`DOWNLOADING`](SlotFlags::DOWNLOADING),
    /// [`LOADED`](SlotFlags::LOADED), [`FAILED`](SlotFlags::FAILED) is set
    /// (none = `Missing`) because every transition `store`s a single state
    /// value. [`SLOW`](SlotFlags::SLOW) is an orthogonal flag OR-ed on top
    /// while the in-flight fetch outlasts the downloader's `soft_timeout`.
    /// A state `store` clears `SLOW` for free, so it is only ever observed
    /// alongside `DOWNLOADING`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct SlotFlags: u8 {
        const DOWNLOADING = 1 << 0;
        const LOADED      = 1 << 1;
        const FAILED      = 1 << 2;
        const SLOW        = 1 << 3;
    }
}

/// Lock-free cache-state discriminant for a segment / init slot.
/// `Downloading` exists to dedupe in-flight fetches: `dispatch` only claims
/// (`Missing -> Downloading`) slots before emitting a `FetchCmd`. The settle
/// path drives `Downloading -> Loaded` (success or "another writer already
/// committed"), `Downloading -> Missing` (recoverable failure / cancel), and
/// `Downloading -> Failed` (terminal: the downloader exhausted its retry
/// budget). Eviction is the only producer of `Loaded -> Missing`.
///
/// `Failed` is terminal by construction: `try_claim` only CAS's from
/// `Missing`, so a failed slot is never re-dispatched (no extra scheduler
/// check needed) and a reader observing it via `is_failed` surfaces a
/// terminal error instead of spinning.
///
/// The only mutators are the typed transitions on the phase-specific
/// `impl FetchClaim<Downloading>` / `impl FetchClaim<Loaded>` blocks (plus
/// the `on_slow` hook), so there is no silent fallback. Reads stay a plain
/// atomic (no lock) because `download_head` scans every slot on the ABR tick.
#[derive(Debug)]
pub(crate) struct SegmentSlotState {
    /// Whether a parked read needs the in-flight fetch's bytes. Set only
    /// while `DOWNLOADING`, cleared by every terminal transition, read back
    /// by the command's live demand probe. Kept beside the flags for the
    /// same reason as `acquire_failures`: a bit packed in there would break
    /// the claim CAS.
    reader_demand: AtomicBool,
    /// Consecutive dispatch-side acquire failures on this slot, kept beside
    /// the flags rather than inside them: [`Self::try_claim`] CAS's the flag
    /// byte against a bare zero, so a counter packed in there would make the
    /// slot unclaimable for good. Survives the `Downloading -> Missing`
    /// requeue — that is the whole point — and is cleared the moment an
    /// acquire succeeds.
    acquire_failures: AtomicU8,
    flags: AtomicU8,
}

impl SegmentSlotState {
    /// Forget the acquire failures: the resource opened, so whatever was in
    /// the way is gone and the next obstruction starts its own budget.
    pub(crate) fn clear_acquire_failures(&self) {
        self.acquire_failures.store(0, Ordering::Release);
    }

    fn flags(&self) -> SlotFlags {
        SlotFlags::from_bits_truncate(self.flags.load(Ordering::Acquire))
    }

    /// True while a parked read needs the current in-flight fetch's bytes.
    pub(crate) fn is_reader_demanded(&self) -> bool {
        self.is_downloading() && self.reader_demand.load(Ordering::Acquire)
    }

    pub(crate) fn mark_failed(&self) {
        self.settle(SlotFlags::FAILED);
    }

    pub(crate) fn mark_loaded(&self) {
        self.settle(SlotFlags::LOADED);
    }

    pub(crate) fn mark_missing(&self) {
        self.settle(SlotFlags::empty());
    }

    /// Mark the in-flight fetch slow (the `on_slow` hook fired). Idempotent;
    /// the next state `store` (terminal transition or fresh claim) clears it.
    pub(crate) fn mark_slow(&self) {
        self.flags
            .fetch_or(SlotFlags::SLOW.bits(), Ordering::AcqRel);
    }

    pub(crate) fn missing() -> Arc<Self> {
        Arc::new(Self {
            flags: AtomicU8::new(SlotFlags::empty().bits()),
            acquire_failures: AtomicU8::new(0),
            reader_demand: AtomicBool::new(false),
        })
    }

    /// Record one dispatch-side acquire failure and return the running count.
    /// Saturates, so a slot retried past the budget keeps reporting the
    /// budget rather than wrapping back under it.
    pub(crate) fn note_acquire_failure(&self) -> u8 {
        let prior = self.acquire_failures.load(Ordering::Acquire);
        let next = prior.saturating_add(1);
        self.acquire_failures.store(next, Ordering::Release);
        next
    }

    /// File a parked read on the in-flight fetch. A slot that is not
    /// `DOWNLOADING` is left alone: a planned one is owed, and the owed
    /// dispatch stamps it `High` at emit.
    pub(crate) fn note_reader_demand(&self) {
        if self.is_downloading() {
            self.reader_demand.store(true, Ordering::Release);
        }
    }

    fn settle(&self, state: SlotFlags) {
        self.flags.store(state.bits(), Ordering::Release);
        self.reader_demand.store(false, Ordering::Release);
    }

    /// Atomic `Missing -> Downloading` claim. Returns the owned
    /// [`FetchClaim<Downloading>`](FetchClaim) handle when the caller now owns
    /// the in-flight slot, `None` when another caller already claimed it.
    /// `plan_revision` records the plan the fetch was taken from, so a
    /// cancelled settle can tell a still-current plan from a superseded one.
    pub(crate) fn try_claim(
        self: &Arc<Self>,
        planned: PlannedFetch,
        plan_revision: PlanRevision,
        variant: Weak<HlsVariant>,
        signal: SizeSignal,
    ) -> Option<FetchClaim<Downloading>> {
        self.flags
            .compare_exchange(
                SlotFlags::empty().bits(),
                SlotFlags::DOWNLOADING.bits(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()
            .map(|_| FetchClaim::claim(planned, plan_revision, variant, Arc::clone(self), signal))
    }

    delegate::delegate! {
        to self {
            #[expr($.contains(SlotFlags::DOWNLOADING))]
            #[call(flags)]
            pub(crate) fn is_downloading(&self) -> bool;
            /// Terminal-failure probe. A `Failed` slot will never load (the
            /// downloader gave up); readers surface a terminal error on it.
            #[expr($.contains(SlotFlags::FAILED))]
            #[call(flags)]
            pub(crate) fn is_failed(&self) -> bool;
            #[expr($.contains(SlotFlags::LOADED))]
            #[call(flags)]
            pub(crate) fn is_loaded(&self) -> bool;
            /// True while the current in-flight fetch has crossed `soft_timeout`
            /// without settling. Meaningful only together with [`Self::is_downloading`].
            #[expr($.contains(SlotFlags::SLOW))]
            #[call(flags)]
            pub(crate) fn is_slow(&self) -> bool;
        }
    }
}

mod sealed {
    pub(crate) trait Sealed {}
}

/// Compile-time download phase of a segment / init slot. The phantom
/// parameter on [`FetchClaim`](crate::segment::FetchClaim) encodes which transitions are legal, so the
/// invariants that `SegmentSlotState` used to check at runtime become
/// type errors: only a `FetchClaim<Downloading>` can settle, and it settles
/// by consuming itself into a `FetchClaim<Loaded>` or `FetchClaim<Missing>`.
///
/// Sealed — the phase set is closed to this module. Each phase carries its
/// own [`Data`](SegmentPhase::Data) payload; phases without state use `()`.
pub(crate) trait SegmentPhase: sealed::Sealed {
    type Data;
}

/// In-flight: claimed via a `Missing -> Downloading` CAS, fetch pending.
pub(crate) struct Downloading;
/// Committed on disk; carries the resolved `final_len`.
pub(crate) struct Loaded;
/// Returned to the dispatch pool (recoverable failure / cancel / evict).
pub(crate) struct Missing;
/// Terminal: the downloader exhausted its retry budget on this slot. Never
/// re-dispatched (`try_claim` only CAS's from `Missing`) and surfaced to
/// readers as a terminal error via [`SegmentSlotState::is_failed`].
pub(crate) struct Failed;

impl sealed::Sealed for Downloading {}
impl sealed::Sealed for Loaded {}
impl sealed::Sealed for Missing {}
impl sealed::Sealed for Failed {}

impl SegmentPhase for Downloading {
    type Data = super::fetch::DownloadClaim;
}
impl SegmentPhase for Loaded {
    type Data = super::fetch::LoadedProof;
}
impl SegmentPhase for Missing {
    type Data = ();
}
impl SegmentPhase for Failed {
    type Data = ();
}
