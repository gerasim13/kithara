use std::{
    io::Error as IoError,
    marker::PhantomData,
    sync::{
        Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use kithara_assets::{AssetReader, AssetWriter, RawWriteHandle, ReadSide, WriteSide};
use kithara_bufpool::HasPool;
use kithara_download::{OnCompleteFn, WriterFn};
use kithara_events::EventBus;
use kithara_net::NetError;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_storage::ResourceStatus;
use tracing::{debug, error};

use crate::{
    DrmEvent, HlsEvent, HlsFailure,
    segment::state::{Downloading, Failed, Loaded, Missing, SegmentPhase, SegmentSlotState},
    signal::SizeSignal,
    variant::{HlsVariant, PlanRevision},
};

/// Whether a fetch error is terminal for the slot — whether this segment is
/// unobtainable, not whether this download failed.
///
/// The net layer spends its budget on one download in well under a second,
/// while the slot it parks may not be read for another half a minute, so the
/// question is [`NetError::can_answer_later`]: a host that refused or was not
/// there returns the slot to the pool and the next dispatch asks again, which
/// is what carries playback through an outage.
///
/// Everything else parks the slot, and a stalled transfer deliberately among
/// them: this is where give-up authority lives for every blocking read above
/// (`impl Read for Stream` waits on the source precisely because this layer can
/// tell a slow-but-live transfer from one that stopped), so a segment whose body
/// never arrives has to end here rather than wait forever.
///
/// `Cancelled` is the exception in the other direction: a cancel marks an epoch
/// rebuild, which owns the re-dispatch.
fn is_terminal_fetch_error(e: &NetError) -> bool {
    !matches!(e, NetError::Cancelled) && !e.can_answer_later()
}

/// Phantom-typed handle to a segment / init slot. `S` is one of
/// [`Downloading`], [`Loaded`], [`Missing`]; the per-phase fields live in
/// `S::Data`. Transitions are consume-self methods on the phase-specific
/// `impl` blocks below, so the compiler rejects a double settle or an
/// [`apply_commit`](crate::variant::HlsVariant::apply_commit) on anything but a `Loaded`
/// handle.
pub(crate) struct FetchClaim<P, S>
where
    P: SegmentPhase<S>,
    S: HasPool<u8> + Send + Sync + 'static,
{
    data: P::Data,
    _schema: PhantomData<fn() -> S>,
}

/// Backing payload of a [`FetchClaim<Downloading>`](FetchClaim). Shares the slot
/// CAS cell so a terminal transition can flip it, holds the `Weak`
/// back-reference for the post-commit size apply, and carries the `Drop`
/// disarm flag.
pub(crate) struct DownloadClaim<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    slot: Arc<SegmentSlotState>,
    plan_revision: PlanRevision,
    planned: PlannedFetch,
    /// Peer-wake handle for the `Drop` recovery: a claim dropped without a
    /// settle must wake the peer to take the requeued work, and the claim
    /// outlives every context that could lend it one.
    signal: SizeSignal,
    variant: Weak<HlsVariant<S>>,
    settled: bool,
}

/// Backing payload of a [`FetchClaim<Loaded>`](FetchClaim): the committed slot
/// identity and resolved size consumed by [`HlsVariant::apply_commit`](crate::variant::HlsVariant::apply_commit).
pub(crate) struct LoadedProof {
    planned: PlannedFetch,
    final_len: u64,
}

impl<S> FetchClaim<Downloading, S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
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
    pub(crate) const fn claim(
        planned: PlannedFetch,
        plan_revision: PlanRevision,
        variant: Weak<HlsVariant<S>>,
        slot: Arc<SegmentSlotState>,
        signal: SizeSignal,
    ) -> Self {
        Self {
            data: DownloadClaim {
                planned,
                plan_revision,
                variant,
                slot,
                signal,
                settled: false,
            },
            _schema: PhantomData,
        }
    }

    /// `Downloading -> Failed` terminal settle: the downloader exhausted
    /// its retry budget (the net layer's resilient body already retried
    /// the stall/transient errors), so the slot is parked permanently —
    /// `try_claim` will not re-dispatch it and a waiting reader surfaces a
    /// terminal error. Unlike [`into_missing`](Self::into_missing) the
    /// slot does NOT return to the dispatch pool.
    pub(crate) fn into_failed(mut self) -> FetchClaim<Failed, S> {
        self.data.slot.mark_failed();
        self.data.settled = true;
        FetchClaim {
            data: (),
            _schema: PhantomData,
        }
    }

    /// `Downloading -> Loaded` with a post-commit size apply. `actual` is
    /// the on-disk `final_len` (success / cache-hit / committed-by-race).
    /// `apply_commit` shrinks the variant's layout to match *before*
    /// `mark_loaded` flips the slot — a reader that observes `Loaded` then
    /// reads the size must never see the stale estimate.
    pub(crate) fn into_loaded(mut self, actual: u64) -> FetchClaim<Loaded, S> {
        let loaded = FetchClaim {
            data: LoadedProof {
                planned: self.data.planned,
                final_len: actual,
            },
            _schema: PhantomData,
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
    pub(crate) fn into_loaded_no_apply(mut self) -> FetchClaim<Loaded, S> {
        self.data.slot.mark_loaded();
        self.data.settled = true;
        FetchClaim {
            data: LoadedProof {
                planned: self.data.planned,
                final_len: 0,
            },
            _schema: PhantomData,
        }
    }

    /// `Downloading -> Missing` recovery (recoverable failure / cancel
    /// before commit). The slot returns to the dispatch pool.
    pub(crate) fn into_missing(mut self) -> FetchClaim<Missing, S> {
        self.data.slot.mark_missing();
        self.data.settled = true;
        FetchClaim {
            data: (),
            _schema: PhantomData,
        }
    }

    /// Share the slot's CAS cell so the `on_slow` hook can flag the in-flight
    /// fetch slow without owning the claim. Cloned before the claim moves into
    /// the `FetchSlot`'s `on_complete`.
    pub(crate) fn slot_state(&self) -> Arc<SegmentSlotState> {
        Arc::clone(&self.data.slot)
    }

    pub(crate) fn variant(&self) -> Option<Arc<HlsVariant<S>>> {
        self.data.variant.upgrade()
    }

    delegate::delegate! {
        to self.data {
            #[field]
            pub(crate) const fn planned(&self) -> PlannedFetch;
            #[field]
            pub(crate) const fn plan_revision(&self) -> PlanRevision;
        }
    }
}

impl<S> FetchClaim<Loaded, S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    delegate::delegate! {
        to self.data {
            #[field]
            pub(crate) const fn final_len(&self) -> u64;
            #[field]
            pub(crate) const fn planned(&self) -> PlannedFetch;
        }
    }
}

/// The `Drop` safety net lives on the concrete payload (not on the generic
/// `FetchClaim<Downloading>`, which `Drop` cannot specialize): if a claim is
/// dropped without a transition, the slot reverts to `Missing` so a leaked
/// handle can never strand it in `Downloading`. The consume-self
/// transitions set `settled` first, disarming this no-op.
///
/// Reverting the slot alone is not recovery: dispatch popped the plan entry
/// when it sent this fetch, so a slot returned to `Missing` describes work
/// nobody holds — the segment is never asked for again and the reader waits
/// forever on the gap (the transient-failure settle documents the same
/// contract). The work goes back on the plan and the peer is woken to take
/// it.
impl<S> Drop for DownloadClaim<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Logs at debug rather than warn: a cancelled or superseded fetch drops its claim on every
    /// seek and variant switch, so this fires by the thousands in healthy stress runs, not just
    /// incidents.
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.slot.mark_missing();
        let requeued = self
            .variant
            .upgrade()
            .is_some_and(|variant| variant.requeue_planned(self.planned, self.plan_revision));
        self.signal.wake_peer();
        debug!(
            target: "kithara_hls::settle",
            planned = ?self.planned,
            requeued,
            "Downloading claim dropped without settle — slot reverted to Missing"
        );
    }
}

/// One unit of pending fetch work for the variant. `Init` is the only
/// non-segment entry — placed at the front of the queue by `rebuild` so
/// the fMP4 init prefix is fetched before any media segment. The derived
/// order (`Init` first, segments ascending) is the plan order the queue
/// keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PlannedFetch {
    Init,
    Segment(u32),
}

/// Fetch ownership and epoch state settled back into one variant slot.
/// Its weak variant reference does not extend the variant lifetime.
pub(crate) struct FetchSlot<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Read view of the writer's generation — used to observe a
    /// committed-by-race status before deciding the terminal transition.
    pub(crate) reader: AssetReader<S>,
    /// Sole commit owner (non-`Clone`); consumed in `settle`.
    pub(crate) writer: AssetWriter<S>,
    pub(crate) cancel: CancelToken,
    pub(crate) bus: EventBus,
    pub(crate) handle: FetchClaim<Downloading, S>,
    /// Clone-able streaming-write handle for the fetch body closure.
    pub(crate) raw: RawWriteHandle,
    /// Unified reader-wake handle — [`SizeSignal::fire`]d on every terminal
    /// settle (commit/fail/cancel) so an off-RT reader parked in
    /// `wait_range(_, None)` re-probes the now-resolved range and the RT
    /// decoder's audio worker re-ticks the instant a commit makes bytes
    /// readable (the decrypt gate opens here for DRM segments), not on its
    /// 10 ms poll.
    pub(crate) signal: SizeSignal,
}

impl<S> From<FetchSlot<S>> for OnCompleteFn
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn from(slot: FetchSlot<S>) -> Self {
        Box::new(move |bytes_written, _headers, err| slot.settle(bytes_written, err))
    }
}

impl<S> FetchSlot<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// On success, commits the resource. `bytes_written` is forwarded as
    /// `final_len` — required by [`ProcessedResource::commit`] to trigger
    /// the post-write decrypt pass on encrypted segments (passing `None`
    /// silently skips decryption, leaving ciphertext on disk). After a
    /// successful commit we read back the resource's `final_len` and
    /// shrink the variant's layout to match: for DRM segments PKCS7
    /// strips up to 16 bytes off the encrypted size, so HEAD-based
    /// estimates are always upper bounds.
    ///
    /// Consumes the slot (`OnCompleteFn` is `FnOnce`): the owned
    /// [`FetchClaim<Downloading>`](FetchClaim) handle is moved into exactly one
    /// terminal transition, so the slot state can never be double-driven.
    fn settle(self, bytes_written: u64, err: Option<&NetError>) {
        let signal = self.signal.clone();
        self.settle_inner(bytes_written, err);
        signal.fire();
    }

    /// Dropping after a newer epoch's writer commits is race-safe (skipped once state is
    /// `Committed`). The work returns to the plan unstamped, since stamping would fail the next
    /// writer's first `write_at`.
    fn settle_cancelled(self, bytes_written: u64) {
        let Self {
            handle,
            writer,
            reader,
            signal,
            ..
        } = self;
        let committed = matches!(reader.status(), ResourceStatus::Committed { .. });
        debug!(
            target: "kithara_hls::settle",
            planned = ?handle.planned(),
            bytes_written,
            committed,
            "stale (cancelled)"
        );
        if committed {
            drop(writer);
            handle.abandon();
        } else {
            let planned = handle.planned();
            let plan_revision = handle.plan_revision();
            let variant = handle.variant();
            writer.abandon();
            handle.into_missing();
            if let Some(variant) = variant
                && variant.requeue_planned(planned, plan_revision)
            {
                signal.wake_peer();
            }
        }
    }

    /// An uncommitted writer is dropped (race-safe cleanup) and the on-disk length adopted if a
    /// newer epoch already committed. Otherwise the segment is requeued onto the plan so a peer
    /// refetches it.
    fn settle_failure(self, e: &NetError) {
        let Self {
            handle,
            writer,
            reader,
            bus,
            signal,
            ..
        } = self;
        let committed = matches!(reader.status(), ResourceStatus::Committed { .. });
        debug!(target: "kithara_hls::settle", err = %e, committed, "fail-path");
        if committed {
            drop(writer);
            if let ResourceStatus::Committed { final_len: Some(n) } = reader.status() {
                handle.into_loaded(n);
            } else {
                handle.into_loaded_no_apply();
            }
        } else {
            writer.fail(e.to_string());
            if is_terminal_fetch_error(e) {
                error!(
                    target: "kithara_hls::settle",
                    err = %e,
                    "terminal fetch failure — slot parked Failed, will not re-dispatch"
                );
                bus.publish(HlsEvent::Error {
                    error: HlsFailure::Other("segment fetch failed".to_string()),
                });
                handle.into_failed();
            } else {
                let planned = handle.planned();
                let plan_revision = handle.plan_revision();
                let variant = handle.variant();
                handle.into_missing();
                if let Some(variant) = variant {
                    let _ = variant.requeue_planned(planned, plan_revision);
                }
                signal.wake_peer();
            }
        }
    }

    fn settle_inner(self, bytes_written: u64, err: Option<&NetError>) {
        if self.cancel.is_cancelled() {
            self.settle_cancelled(bytes_written);
            return;
        }
        match err {
            None => self.settle_success(bytes_written),
            Some(e) => self.settle_failure(e),
        }
    }

    /// Reads `final_len` back off the committed reader rather than trusting `bytes_written`, since
    /// PKCS7 unpadding shrinks DRM segments below their announced size.
    fn settle_success(self, bytes_written: u64) {
        let Self {
            handle,
            writer,
            bus,
            ..
        } = self;
        let planned = handle.planned();
        let variant = handle.variant();
        match writer.commit(Some(bytes_written)) {
            Ok(reader) => {
                debug!(target: "kithara_hls::settle", bytes_written, "success");
                let actual = match reader.status() {
                    ResourceStatus::Committed { final_len: Some(n) } => n,
                    _ => bytes_written,
                };
                handle.into_loaded(actual);
                if let Some(variant) = variant {
                    variant.maybe_publish_cache_complete();
                }
            }
            Err(e) => {
                debug!(
                    target: "kithara_hls::settle",
                    bytes_written,
                    err = %e,
                    "success-but-commit-failed"
                );
                if let Some((variant_idx, segment_index)) =
                    decrypt_failure_site(planned, variant.as_ref())
                {
                    bus.publish(DrmEvent::SegmentDecryptFailed {
                        segment_index,
                        variant: variant_idx,
                        detail: e.to_string(),
                    });
                }
                handle.into_missing();
            }
        }
    }

    pub(crate) fn writer(&self) -> WriterFn {
        let raw = self.raw.clone();
        let offset = Arc::new(AtomicU64::new(0));
        Box::new(move |chunk: &[u8]| {
            let pos = offset.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            raw.write_at(pos, chunk).map_err(IoError::other)
        })
    }
}

fn decrypt_failure_site<S>(
    planned: PlannedFetch,
    variant: Option<&Arc<HlsVariant<S>>>,
) -> Option<(u32, u32)>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use kithara_net::NetError;
    use kithara_test_utils::kithara;

    use super::is_terminal_fetch_error;

    fn status(code: u16) -> NetError {
        NetError::Status {
            status: NonZeroU16::new(code).expect("non-zero status"),
            url: None,
            body: None,
        }
    }

    /// Whether a segment is obtainable later is the cause's own retryability,
    /// not the shape the outage arrived in. A transport that died and a server
    /// that said "not now" are both answerable once the network is back, so
    /// both keep the slot; only an answer that cannot change parks it.
    #[kithara::test]
    #[case::transport_gone(NetError::Network("connection closed".to_string()), false)]
    // A body that stopped arriving ends here: the blocking read above this layer
    // has no other bound, which `audio_new_bounded_failure_when_first_segment_withheld`
    // pins.
    #[case::body_stopped_arriving(NetError::Timeout, true)]
    #[case::server_busy(status(503), false)]
    #[case::too_many_requests(status(429), false)]
    #[case::missing_segment(status(404), true)]
    #[case::undecodable_body(NetError::Decode("bad box".to_string()), true)]
    fn exhausted_budget_defers_to_its_cause(#[case] cause: NetError, #[case] terminal: bool) {
        let exhausted = NetError::RetryExhausted {
            max_retries: 3,
            source: Box::new(cause),
        };
        assert_eq!(is_terminal_fetch_error(&exhausted), terminal);
    }

    /// A cancel marks an epoch rebuild, which owns the re-dispatch.
    #[kithara::test]
    fn cancel_keeps_the_slot() {
        assert!(!is_terminal_fetch_error(&NetError::Cancelled));
    }
}
