use std::{marker::PhantomData, sync::Weak};

use kithara_events::{DrmEvent, EventBus, HlsError as EventHlsError, HlsEvent};
use kithara_file::FetchOutcome;
use kithara_net::{NetError, Retryability};
use kithara_platform::sync::{Arc, Mutex};
use tracing::{debug, error, warn};

use crate::{
    segment::{
        core::FetchCell,
        state::{Downloading, Failed, Loaded, Missing, SegmentPhase, SegmentSlotState},
    },
    signal::SizeSignal,
    variant::HlsVariant,
};

/// Whether a fetch error is terminal for the slot. The net layer's
/// resilient body already retried stalls and transient body errors, so a
/// `Fatal` error reaching the settle path means the downloader gave up —
/// the slot must be parked `Failed` rather than re-dispatched. `Cancelled`
/// is the one `Fatal` variant that stays recoverable: a cancel marks an
/// epoch rebuild, which owns the re-dispatch, so it keeps `Missing`.
fn is_terminal_fetch_error(e: &NetError) -> bool {
    !matches!(e, NetError::Cancelled) && e.retryability() == Retryability::Fatal
}

/// Phantom-typed handle to a segment / init slot. `S` is one of
/// [`Downloading`], [`Loaded`], [`Missing`]; the per-phase fields live in
/// `S::Data`. Transitions are consume-self methods on the phase-specific
/// `impl` blocks below, so the compiler rejects a double settle or an
/// [`apply_commit`](crate::variant::HlsVariant::apply_commit) on anything but a `Loaded`
/// handle.
pub(crate) struct FetchClaim<S: SegmentPhase> {
    data: S::Data,
    _phase: PhantomData<S>,
}

/// Backing payload of a [`FetchClaim<Downloading>`](FetchClaim). Shares the slot
/// CAS cell so a terminal transition can flip it, holds the `Weak`
/// back-reference for the post-commit size apply, and carries the `Drop`
/// disarm flag.
pub(crate) struct DownloadClaim {
    slot: Arc<SegmentSlotState>,
    planned: PlannedFetch,
    variant: Weak<HlsVariant>,
    settled: bool,
}

/// Backing payload of a [`FetchClaim<Loaded>`](FetchClaim): the committed slot
/// identity and resolved size consumed by [`HlsVariant::apply_commit`](crate::variant::HlsVariant::apply_commit).
pub(crate) struct LoadedProof {
    planned: PlannedFetch,
    final_len: u64,
}

impl FetchClaim<Downloading> {
    /// Consume the claim without touching slot state — used for a stale
    /// (cancelled) settle whose resource already committed: the new epoch
    /// owns the slot, so leaving it as-is is correct.
    pub(crate) fn abandon(mut self) {
        self.data.settled = true;
    }

    /// Build the owned in-flight handle after [`SegmentSlotState::try_claim`]
    /// wins the `Missing -> Downloading` CAS. `slot` shares the just-flipped
    /// CAS cell so a terminal transition can settle it; `variant` is the
    /// `Weak` back-reference for the post-commit size apply.
    pub(crate) fn claim(
        planned: PlannedFetch,
        variant: Weak<HlsVariant>,
        slot: Arc<SegmentSlotState>,
    ) -> Self {
        Self {
            data: DownloadClaim {
                planned,
                variant,
                slot,
                settled: false,
            },
            _phase: PhantomData,
        }
    }

    /// `Downloading -> Failed` terminal settle: the downloader exhausted
    /// its retry budget (the net layer's resilient body already retried
    /// the stall/transient errors), so the slot is parked permanently —
    /// `try_claim` will not re-dispatch it and a waiting reader surfaces a
    /// terminal error. Unlike [`into_missing`](Self::into_missing) the
    /// slot does NOT return to the dispatch pool.
    pub(crate) fn into_failed(mut self) -> FetchClaim<Failed> {
        self.data.slot.mark_failed();
        self.data.settled = true;
        FetchClaim {
            data: (),
            _phase: PhantomData,
        }
    }

    /// `Downloading -> Loaded` with a post-commit size apply. `actual` is
    /// the on-disk `final_len` (success / cache-hit / committed-by-race).
    /// `apply_commit` shrinks the variant's layout to match *before*
    /// `mark_loaded` flips the slot — a reader that observes `Loaded` then
    /// reads the size must never see the stale estimate.
    pub(crate) fn into_loaded(mut self, actual: u64) -> FetchClaim<Loaded> {
        let loaded = FetchClaim {
            data: LoadedProof {
                planned: self.data.planned,
                final_len: actual,
            },
            _phase: PhantomData,
        };
        if let Some(v) = self.data.variant.upgrade() {
            v.apply_commit(&loaded);
        }
        self.data.slot.mark_loaded();
        self.data.settled = true;
        loaded
    }

    /// `Downloading -> Loaded` without a size apply — the resource
    /// committed by a racing writer but reported no `final_len`, so the
    /// existing layout estimate stands.
    pub(crate) fn into_loaded_no_apply(mut self) -> FetchClaim<Loaded> {
        self.data.slot.mark_loaded();
        self.data.settled = true;
        FetchClaim {
            data: LoadedProof {
                planned: self.data.planned,
                final_len: 0,
            },
            _phase: PhantomData,
        }
    }

    /// `Downloading -> Missing` recovery (recoverable failure / cancel
    /// before commit). The slot returns to the dispatch pool.
    pub(crate) fn into_missing(mut self) -> FetchClaim<Missing> {
        self.data.slot.mark_missing();
        self.data.settled = true;
        FetchClaim {
            data: (),
            _phase: PhantomData,
        }
    }

    /// Share the slot's CAS cell so the `on_slow` hook can flag the in-flight
    /// fetch slow without owning the claim.
    pub(crate) fn slot_state(&self) -> Arc<SegmentSlotState> {
        Arc::clone(&self.data.slot)
    }

    pub(crate) fn planned(&self) -> PlannedFetch {
        self.data.planned
    }

    pub(crate) fn variant(&self) -> Option<Arc<HlsVariant>> {
        self.data.variant.upgrade()
    }
}

impl FetchClaim<Loaded> {
    delegate::delegate! {
        to self.data {
            #[field]
            pub(crate) fn final_len(&self) -> u64;
            #[field]
            pub(crate) fn planned(&self) -> PlannedFetch;
        }
    }
}

/// The `Drop` safety net lives on the concrete payload (not on the generic
/// `FetchClaim<Downloading>`, which `Drop` cannot specialize): if a claim is
/// dropped without a transition, the slot reverts to `Missing` so a leaked
/// handle can never strand it in `Downloading`. The consume-self
/// transitions set `settled` first, disarming this no-op.
impl Drop for DownloadClaim {
    fn drop(&mut self) {
        if !self.settled {
            self.slot.mark_missing();
            warn!(
                target: "kithara_hls::settle",
                planned = ?self.planned,
                "Downloading claim dropped without settle — slot reverted to Missing"
            );
        }
    }
}

/// One unit of pending fetch work for the variant. `Init` is the only
/// non-segment entry — placed at the front of the queue by `rebuild` so
/// the fMP4 init prefix is fetched before any media segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedFetch {
    Init,
    Segment(u32),
}

/// Turns the outcome of a segment's file fetch into the slot transition it
/// implies. The file owned the bytes; this owns the timeline.
pub(crate) struct SegmentSettle<'a> {
    bus: &'a EventBus,
    cell: &'a FetchCell,
    claim: &'a Mutex<Option<FetchClaim<Downloading>>>,
    signal: &'a SizeSignal,
}

impl<'a> SegmentSettle<'a> {
    pub(crate) fn new(
        claim: &'a Mutex<Option<FetchClaim<Downloading>>>,
        cell: &'a FetchCell,
        signal: &'a SizeSignal,
        bus: &'a EventBus,
    ) -> Self {
        Self {
            bus,
            cell,
            claim,
            signal,
        }
    }

    /// Settle the slot, then wake. The wake comes AFTER the terminal
    /// transition (commit makes bytes readable / fail flips
    /// `range_has_failed`) and re-ticks the RT decoder's audio worker: for
    /// DRM the decrypted bytes only become readable at commit, so this —
    /// not the ciphertext write — is the load-bearing wake.
    pub(crate) fn apply(self, outcome: FetchOutcome) {
        let Some(handle) = self.claim.lock().take() else {
            debug!(target: "kithara_hls::settle", "outcome for an already-settled claim");
            return;
        };
        self.transition(handle, outcome);
        self.signal.fire();
        // The fetch is over, so the file goes — and with it the peer
        // registration. Taken out of the cell first so the drop runs
        // without the cell lock held.
        drop(self.cell.lock().take());
    }

    fn transition(&self, handle: FetchClaim<Downloading>, outcome: FetchOutcome) {
        match outcome {
            FetchOutcome::Committed { final_len } => Self::committed(handle, final_len),
            FetchOutcome::Cancelled { committed } => Self::cancelled(handle, committed),
            FetchOutcome::Failed { error } => self.failed(handle, &error),
            FetchOutcome::CommitFailed { reason } => self.commit_failed(handle, reason),
            // An outcome this crate does not know is not a transition it
            // can make: dropping the claim reverts the slot to `Missing`,
            // which keeps it re-dispatchable.
            _ => warn!(target: "kithara_hls::settle", "unrecognised fetch outcome"),
        }
    }

    /// A length means the resource committed here and the variant's layout
    /// shrinks to it; no length means a racing writer committed and reported
    /// none, so the existing estimate stands.
    fn committed(handle: FetchClaim<Downloading>, final_len: Option<u64>) {
        let Some(actual) = final_len else {
            debug!(target: "kithara_hls::settle", "committed by a racing writer");
            handle.into_loaded_no_apply();
            return;
        };
        debug!(target: "kithara_hls::settle", actual, "committed");
        let variant = handle.variant();
        handle.into_loaded(actual);
        if let Some(variant) = variant {
            variant.maybe_publish_cache_complete();
        }
    }

    /// A cancel whose resource committed belongs to the new epoch's writer —
    /// it owns the slot now, so leaving it alone is the correct settle.
    fn cancelled(handle: FetchClaim<Downloading>, committed: bool) {
        if committed {
            debug!(target: "kithara_hls::settle", "stale (cancelled), resource committed");
            handle.abandon();
        } else {
            debug!(target: "kithara_hls::settle", "stale (cancelled) before commit");
            handle.into_missing();
        }
    }

    /// The bytes arrived but the commit rejected them — for a DRM segment
    /// that is a decrypt failure. Recoverable: the slot returns to the
    /// dispatch pool.
    fn commit_failed(&self, handle: FetchClaim<Downloading>, reason: String) {
        debug!(target: "kithara_hls::settle", reason, "commit rejected the bytes");
        if let Some((variant, segment_index)) =
            decrypt_failure_site(handle.planned(), handle.variant().as_ref())
        {
            self.bus.publish(DrmEvent::SegmentDecryptFailed {
                variant,
                segment_index,
                detail: reason,
            });
        }
        handle.into_missing();
    }

    /// The net layer's resilient body already retried stalls and transient
    /// body errors; a `Fatal` error here (e.g. `RetryExhausted`) means the
    /// downloader gave up, so the slot is terminal — parking it as `Failed`
    /// stops the re-dispatch loop and lets a waiting reader surface a
    /// terminal error. The typed cause is logged here; readers see only the
    /// fixed terminal message (no transport detail).
    fn failed(&self, handle: FetchClaim<Downloading>, error: &NetError) {
        if !is_terminal_fetch_error(error) {
            debug!(target: "kithara_hls::settle", err = %error, "recoverable fetch failure");
            handle.into_missing();
            return;
        }
        error!(
            target: "kithara_hls::settle",
            err = %error,
            "terminal fetch failure — slot parked Failed, will not re-dispatch"
        );
        self.bus.publish(HlsEvent::Error {
            error: EventHlsError::Other("segment fetch failed".to_string()),
        });
        handle.into_failed();
    }
}

fn decrypt_failure_site(
    planned: PlannedFetch,
    variant: Option<&Arc<HlsVariant>>,
) -> Option<(u32, u32)> {
    let PlannedFetch::Segment(segment_index) = planned else {
        return None;
    };
    let variant = variant?;
    if !variant.is_encrypted_segment(segment_index) {
        return None;
    }
    let variant_idx = variant.variant_index_u32()?;
    Some((variant_idx, segment_index))
}
