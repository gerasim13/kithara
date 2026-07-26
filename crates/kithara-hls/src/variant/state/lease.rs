use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::SeekObserve;

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
/// it being moved right now — is answered against the timeline that moves it.
pub(super) struct Cursor {
    position: AtomicU64,
    seek_obs: Arc<dyn SeekObserve>,
}

impl Cursor {
    fn new(seek_obs: Arc<dyn SeekObserve>) -> Self {
        Self {
            seek_obs,
            position: AtomicU64::new(0),
        }
    }

    pub(super) fn advance(&self, n: u64) {
        self.position.fetch_add(n, Ordering::AcqRel);
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

    pub(super) fn set_position(&self, pos: u64) {
        self.position.store(pos, Ordering::Release);
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
            variant,
        })
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
    cursor: Arc<Cursor>,
    leases: Box<[Arc<ReadLease>]>,
}

impl ReadSession {
    pub(crate) fn new(seek_obs: Arc<dyn SeekObserve>, variants: &[Arc<HlsVariant>]) -> Self {
        let cursor = Arc::new(Cursor::new(seek_obs));
        let leases = variants
            .iter()
            .map(|variant| ReadLease::new(Arc::clone(variant), Arc::clone(&cursor)))
            .collect();
        Self { cursor, leases }
    }

    pub(crate) fn lease(&self, variant_index: usize) -> Option<&Arc<ReadLease>> {
        self.leases.get(variant_index)
    }

    pub(crate) fn leases(&self) -> impl Iterator<Item = &Arc<ReadLease>> {
        self.leases.iter()
    }

    /// The byte this session has consumed up to — the value a token holder
    /// publishes as the stream position.
    pub(crate) fn position(&self) -> u64 {
        self.cursor.position()
    }
}
