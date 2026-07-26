use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::{Publication, SeekObserve, SessionId};

use super::{
    HlsVariant,
    cas_anchor::CasAnchorCell,
    probe::SizeDemandState,
    seqlock::{AtomicOptU64, AtomicSeekAlias},
};
use crate::segment::PlannedFetch;

/// The byte cursor one reading session consumes the stream through.
///
/// Shared by every lease the session holds: the session reads one stream, so
/// crossing a variant boundary must not restart its progress. The seek handle
/// travels with it because every question about the cursor — has it moved, is
/// it being moved right now, which timeline does this byte belong to — is
/// answered against the timeline that moves it.
///
/// The cursor is private. What the stream reports as its position is the
/// [`Publication`], and only the session holding its token feeds it; a session
/// being built ahead of time reads through its own cursor and publishes
/// nothing.
pub(super) struct Cursor {
    position: AtomicU64,
    seek_obs: Arc<dyn SeekObserve>,
    session: SessionId,
    publication: Arc<Publication>,
}

impl Cursor {
    fn new(
        seek_obs: Arc<dyn SeekObserve>,
        session: SessionId,
        publication: Arc<Publication>,
    ) -> Self {
        Self {
            seek_obs,
            session,
            publication,
            position: AtomicU64::new(0),
        }
    }

    pub(super) fn advance(&self, n: u64) {
        let to = self
            .position
            .fetch_add(n, Ordering::AcqRel)
            .saturating_add(n);
        self.publication
            .advance(self.session, to, self.seek_obs.epoch());
    }

    pub(super) fn is_flushing(&self) -> bool {
        self.seek_obs.is_flushing()
    }

    pub(super) fn is_seek_active(&self) -> bool {
        self.seek_obs.is_flushing() || self.seek_obs.is_pending()
    }

    pub(super) fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    /// Move the reader: a seek, a commit re-aiming it, or an anchor resolved
    /// once the exact byte prefix arrived. The projection follows wherever it
    /// went, which is why this rebases rather than advances.
    pub(super) fn set_position(&self, pos: u64) {
        self.position.store(pos, Ordering::Release);
        self.publication
            .rebase(self.session, pos, self.seek_obs.epoch());
    }
}

/// One reading session's claim on one variant: everything about fetching that
/// depends on where THIS reader is.
///
/// The variant underneath owns what every reader of it shares — the segment
/// table, the byte layout, the slots a fetch claims. What a reader cannot
/// share is here: what it still wants fetched, how far ahead to fetch, which
/// exact sizes it is waiting on before it can address a byte, and the alias
/// that stands in for an anchor until those sizes resolve. Two readers on two
/// variants therefore plan without meeting, which is what lets a transition
/// build its incoming side while the outgoing side keeps playing.
/// Reading a lease reads its variant: the geometry questions — where a
/// segment sits, how long the stream is, whether a slot is loaded — are the
/// variant's to answer, and a lease never shadows one of them.
#[derive(derive_more::Deref)]
pub(crate) struct ReadLease {
    #[deref]
    pub(super) variant: Arc<HlsVariant>,
    pub(super) cursor: Arc<Cursor>,
    pub(super) prefetch_anchor: AtomicU64,
    pub(super) queue: Mutex<VecDeque<PlannedFetch>>,
    /// Lock-free, allocation-free exact-byte-seek demand, read and cleared on
    /// the produce-core metadata-phase gate
    /// ([`ReadLease::exact_byte_metadata_phase`]). A single `AtomicU64` with a
    /// `u64::MAX` none sentinel.
    pub(super) exact_byte_seek: AtomicOptU64,
    /// Lock-free, allocation-free seek-alias snapshot. The produce-core read
    /// path ([`ReadLease::seek_alias_at`], reached on every `find_at_offset`)
    /// and the steady-read-path clear (`advance`) touch only atomics; the base
    /// is single-writer (on-core), the resolved exact anchor is published
    /// off-RT under a generation tag. See `flow/seqlock.rs`.
    pub(super) alias: AtomicSeekAlias,
    pub(super) segment_aware_tail: AtomicU32,
    /// Lock-free, allocation-free exact-seek demand, read on the produce-core
    /// metadata-phase gate ([`ReadLease::exact_seek_metadata_phase`]). Body is
    /// MULTI-writer: the on-core seek path (`seek_time_anchor`) and the off-RT
    /// downloader seek-epoch reset (`rebuild_at_time`) both SET it with no lock
    /// between them, so the cell serializes writers with a CAS-acquired version
    /// and the RT reader bails to not-ready (never spins) on a write-in-flight.
    /// Off-RT completers CAS-consume the generation.
    pub(super) exact_seek: CasAnchorCell,
    pub(super) size_demand: Mutex<SizeDemandState>,
    /// How far behind this lease is on fetch budget. A lease that had work to
    /// do and got none accrues; every command it emits pays one back. Dispatch
    /// serves the most starved first, so a lease cannot be talked over
    /// indefinitely by a busier one — which is what two readers sharing one
    /// budget need, and what one reader never notices.
    deficit: AtomicUsize,
}

impl ReadLease {
    fn new(variant: Arc<HlsVariant>, cursor: Arc<Cursor>) -> Arc<Self> {
        // Preallocate to the worst-case rebuild size (init + every media
        // segment + the seg-0 decoder probe) so the per-seek `clear` +
        // `extend` in `rebuild_queue` never reallocates.
        let capacity = (variant.num_segments() as usize).saturating_add(2);
        Arc::new(Self {
            cursor,
            prefetch_anchor: AtomicU64::new(0),
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            exact_byte_seek: AtomicOptU64::none(),
            alias: AtomicSeekAlias::new(),
            segment_aware_tail: AtomicU32::new(HlsVariant::NO_SEEK_TAIL),
            exact_seek: CasAnchorCell::new(),
            size_demand: Mutex::default(),
            deficit: AtomicUsize::new(0),
            variant,
        })
    }

    /// Fall further behind, up to `cap` — the bound on how long one lease can
    /// keep priority after being unstuck.
    pub(crate) fn accrue(&self, cap: usize) {
        self.deficit
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                Some(d.saturating_add(1).min(cap))
            })
            .ok();
    }

    /// Pay back one credit per command emitted.
    pub(crate) fn debit(&self, commands: usize) {
        self.deficit
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                Some(d.saturating_sub(commands))
            })
            .ok();
    }

    pub(crate) fn deficit(&self) -> usize {
        self.deficit.load(Ordering::Acquire)
    }

    /// Whether serving this lease could emit anything: a plan entry to claim,
    /// or a size it still has to learn. A lease with nothing to do is skipped
    /// without consuming budget and without falling behind.
    pub(crate) fn has_eligible_work(&self) -> bool {
        !self.queue.lock().is_empty() || self.size_demand.lock().is_pending()
    }

    pub(crate) fn variant(&self) -> &Arc<HlsVariant> {
        &self.variant
    }

    pub(crate) fn variant_index(&self) -> usize {
        self.variant.index()
    }
}

/// One reader's leases over a track, one per variant.
///
/// A session exists for as long as something reads the track through it. At
/// N=1 the track has exactly one, and its lease on the ABR-current variant is
/// the audible one; a transition adds a second session whose leases plan the
/// incoming variant without touching the outgoing plan.
pub(crate) struct ReadSession {
    leases: Box<[Arc<ReadLease>]>,
}

impl ReadSession {
    pub(crate) fn new(
        id: SessionId,
        seek_obs: Arc<dyn SeekObserve>,
        publication: Arc<Publication>,
        variants: &[Arc<HlsVariant>],
    ) -> Self {
        let cursor = Arc::new(Cursor::new(seek_obs, id, publication));
        let leases = variants
            .iter()
            .map(|variant| ReadLease::new(Arc::clone(variant), Arc::clone(&cursor)))
            .collect();
        Self { leases }
    }

    pub(crate) fn lease(&self, variant_index: usize) -> Option<&Arc<ReadLease>> {
        self.leases.get(variant_index)
    }

    pub(crate) fn leases(&self) -> impl Iterator<Item = &Arc<ReadLease>> {
        self.leases.iter()
    }
}
