use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
use kithara_abr::Abr;
use kithara_assets::AssetResource;
use kithara_drm::DecryptContext;
use kithara_events::EventBus;
use kithara_net::Headers;
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
#[cfg(test)]
use kithara_stream::dl::Downloader;
use kithara_stream::{AudioCodec, ContainerFormat, dl::FetchScope};

use super::{cancel_epoch::CancelEpoch, offsets::Layout};
use crate::{
    HlsResult,
    config::SizeProbeMethod,
    playlist::{PlaylistAccess, PlaylistState},
    segment::{
        MediaSegment, Segment, SegmentContent, SegmentSize, SegmentSlotState, SegmentSource,
    },
    signal::SizeSignal,
};

pub(super) const INIT_PLACEHOLDER_BYTES: u64 = 16 * 1024;

/// Fetch scope for a `PlanCtx` built in a unit test: a real pool with an
/// idle stream peer registered in it, so a segment file registers as that
/// peer's child exactly as it does in production. No test in this crate
/// drives it to the network.
#[cfg(test)]
pub(crate) fn test_transport() -> Arc<dyn FetchScope> {
    struct Idle;

    impl Abr for Idle {}

    impl kithara_stream::dl::Peer for Idle {}

    let downloader = Downloader::new(
        kithara_stream::dl::DownloaderConfig::for_client(kithara_net::HttpClient::new(
            kithara_net::NetOptions::default(),
            CancelToken::never(),
        ))
        .build(),
    );
    Arc::new(downloader.register(Arc::new(Idle)))
}

pub(crate) struct PlanCtx {
    pub(crate) bus: EventBus,
    /// The stream's peer, handed down as the scope every segment file
    /// registers in. A segment is an ordinary `kithara-file` file: it
    /// fetches through this pool rather than minting a client of its own,
    /// and registering here is what makes it a piece of the stream's
    /// download — ranked behind it, crediting its bytes to it, and
    /// unregistered when the stream goes away.
    pub(crate) fetch: Arc<dyn FetchScope>,
    pub(crate) scope: kithara_assets::AssetScope,
    pub(crate) master_cancel: CancelToken,
    /// Per-resource HTTP headers applied to every init/segment fetch.
    /// Mirrors `HlsConfig::headers`; threaded through so DRM-style auth
    /// tokens carried by the playlist load also reach segment GETs.
    pub(crate) headers: Option<Headers>,
    /// Max bytes the downloader may be ahead of the reader before
    /// `dispatch` pauses emitting `FetchCmd`s. Mirrors
    /// `HlsConfig::look_ahead_bytes`:
    /// - `Some(n)` — when a segment's start offset exceeds
    ///   `variant.position() + n`, leave it (and everything after)
    ///   in the queue; further prefetch waits for the reader to
    ///   advance.
    /// - `None` — no cap (download as fast as possible).
    ///
    /// `Init` is always emitted regardless — the fMP4 demuxer needs
    /// it before any segment can decode.
    pub(crate) look_ahead_bytes: Option<u64>,
    /// Max media segments the downloader may keep ahead of the reader.
    /// This is derived from small ephemeral cache capacity, where byte-only
    /// lookahead can otherwise prefetch more resources than the cache can retain
    /// and trigger eviction/rebuild thrash.
    pub(crate) look_ahead_segments: Option<usize>,
    pub(crate) size_probe_method: SizeProbeMethod,
    /// Unified reader-wake handle (owned by [`HlsCoord`]). Every `FetchCmd` this
    /// plan emits fires it on each segment byte write and on settle
    /// (commit/fail) — [`SizeSignal::fire`] wakes both the off-RT reader parked
    /// in `wait_range(_, None)` (the moment its range fills, no wall-clock poll)
    /// and the RT decoder's audio worker (re-ticked on data arrival rather than
    /// on its 10 ms scheduler poll). The late-bound worker wake inside is filled
    /// once by `HlsSource::set_worker_wake`; only the two downloader-thread sites
    /// fire it — the coord's RT-reachable fence/seek signals use
    /// [`SizeSignal::fire_ready_only`].
    pub(crate) signal: SizeSignal,
    /// Snapshot of `SeekObserve::epoch()` at plan-time. Tagged on
    /// every emitted `FetchCmd`'s probe so integration tests can
    /// distinguish fetches that pre-date a user seek from those that
    /// the scheduler issued *after* observing the new epoch.
    pub(crate) seek_epoch: u64,
    pub(crate) prefetch_budget: usize,
}

/// Inputs to [`HlsVariant::activate_at_segment_with_shift`]: pin `from_seg`
/// at `seg_boundary` in virtual byte space and move the reader to
/// `reader_pos`.
#[derive(Clone, Copy)]
pub(crate) struct SegmentActivateParams {
    pub(crate) from_seg: u32,
    pub(crate) reader_pos: u64,
    pub(crate) seg_boundary: u64,
}

/// One variant of the track: what every reader of it shares.
///
/// The segment table, the byte layout and the slot a fetch claims are facts
/// about the variant, so they live here. What depends on where a particular
/// reader is — its plan, its prefetch aim, the sizes it is waiting on — is a
/// [`ReadLease`](super::ReadLease) over this variant, not part of it.
pub(crate) struct HlsVariant {
    /// Coherent owner of the cross-variant byte-address-space coordinates
    /// (`byte_shift`, `served_from`, `served_until`, `init_seed`, the media
    /// offset table). A single lock guards all five so a reader never mixes
    /// the shift of one activation with the served bounds of the next.
    pub(super) layout: Layout,
    pub(super) profile: VariantProfile,
    /// The variant's cancel epoch: the per-track parent (mirror of
    /// `coord.cancel` = `PlanCtx::master_cancel`) plus the rotating
    /// per-activation child. Survives variant re-activation — a cross-codec
    /// `commit_variant_switch` may flip from `v_old` to `v_new` and back to
    /// `v_old`, and the second activation of `v_old` must dispatch fetches
    /// under a *live* cancel, so [`HlsVariant::rearm_cancel`] rotates the child.
    pub(super) cancel_epoch: CancelEpoch,
    pub(super) segments: VariantSegments,
    pub(super) variant: usize,
    pub(super) cache_complete_emitted: AtomicBool,
}

pub(super) struct VariantProfile {
    /// Cached audio codec — pulled from `playlist_state` at construction
    /// time. The reader's hot path (`media_info`) reads this without
    /// taking the playlist's per-variant `RwLock`.
    pub(super) codec: Option<AudioCodec>,
    /// Cached container format — see [`Self::codec`].
    pub(super) container: Option<ContainerFormat>,
    /// HTTP headers applied to every `FetchCmd` this variant emits.
    /// Snapshotted from `PlanCtx::headers` at construction; carries
    /// resource-wide auth (e.g. zvuk `X-Auth-Token`) so segment GETs
    /// reach the same authenticated endpoint as the playlist load.
    pub(super) headers: Option<Headers>,
    pub(super) bus: EventBus,
}

#[derive(derive_more::Deref)]
pub(super) struct VariantSegments {
    /// Init slot: `Some(Segment::Init)` for a variant that advertises a
    /// separately fetched `#EXT-X-MAP` init, `None` otherwise. Its existence
    /// is keyed on the playlist `#EXT-X-MAP` URL, never on the known byte size
    /// — see [`Self::build_init_entry`].
    pub(super) init: Option<Segment>,
    /// Media entry table — the fetch path and descriptor builders index it
    /// for `url` / `content` / `decode_time`.
    #[deref]
    entries: Vec<Segment>,
}

impl VariantSegments {
    fn new(init: Option<Segment>, entries: Vec<Segment>) -> Self {
        Self { init, entries }
    }
}

/// Media payload + shared-dep snapshot that owns bare variant assembly.
/// Production builds this from parsed playlist metadata; tests build it
/// inline from synthesised fixtures.
pub(crate) struct VariantParts {
    pub(crate) codec: Option<AudioCodec>,
    pub(crate) container: Option<ContainerFormat>,
    pub(crate) init: Option<Segment>,
    pub(crate) segments: Vec<Segment>,
}

/// In-memory init prefix size: 0 when the variant carries no separately
/// fetched `#EXT-X-MAP` init (`None`), else the init `Segment`'s current
/// (possibly post-commit shrunk) size atom.
fn init_size_of(init: &Option<Segment>) -> u64 {
    init.as_ref().map_or(0, Segment::len)
}

/// Route-only non-exact media placeholder for playlists without byte ranges.
/// `EXTINF` keeps the virtual byte layout proportional to media time without
/// pretending to know payload length. Exactness gates still prevent
/// authoritative EOF and file-like seek math until body commit or a lazy
/// exact-size demand publishes the real length.
pub(super) fn segment_placeholder_size(duration: Duration, bandwidth_bps: Option<u64>) -> u64 {
    const MIN_BYTES: u64 = 4 * 1024;
    const MAX_PRECOMMIT_BYTES: u64 = 256 * 1024;
    const PLACEHOLDER_BYTES_PER_SEC: u128 = 4 * 1024;
    const NANOS_PER_SEC: u128 = 1_000_000_000;
    const BITS_PER_BYTE: u128 = 8;

    let duration_nanos = duration.as_nanos();
    if duration_nanos == 0 {
        return MIN_BYTES;
    }
    let duration_bytes = duration_nanos
        .saturating_mul(PLACEHOLDER_BYTES_PER_SEC)
        .div_ceil(NANOS_PER_SEC);
    let estimated_bytes = bandwidth_bps
        .filter(|bps| *bps > 0)
        .map_or(duration_bytes, |bps| {
            let bandwidth_bytes = u128::from(bps)
                .saturating_mul(duration_nanos)
                .div_ceil(BITS_PER_BYTE * NANOS_PER_SEC);
            duration_bytes.min(bandwidth_bytes)
        });
    u64::try_from(estimated_bytes)
        .unwrap_or(u64::MAX)
        .clamp(MIN_BYTES, MAX_PRECOMMIT_BYTES)
}

pub(crate) struct VariantParams<'a> {
    pub(crate) ctx: &'a PlanCtx,
    pub(crate) decrypt_contexts: &'a [Option<DecryptContext>],
    pub(crate) init_decrypt_ctx: Option<DecryptContext>,
    pub(crate) variant_idx: usize,
}

impl HlsVariant {
    pub(crate) fn try_build(
        playlist_state: &Arc<PlaylistState>,
        params: VariantParams<'_>,
    ) -> HlsResult<Arc<Self>> {
        let VariantParams {
            variant_idx,
            init_decrypt_ctx,
            decrypt_contexts,
            ctx,
        } = params;
        let init =
            Self::build_init_entry(playlist_state.as_ref(), variant_idx, init_decrypt_ctx, ctx)?;
        let segments = Self::build_segment_entries(
            playlist_state.as_ref(),
            decrypt_contexts,
            variant_idx,
            ctx,
        )?;
        let codec = playlist_state.variant_codec(variant_idx);
        let container = playlist_state.variant_container(variant_idx);
        Ok(VariantParts {
            codec,
            container,
            init,
            segments,
        }
        .into_variant(variant_idx, ctx))
    }
}

impl VariantParts {
    /// Bare assembly used by unit tests inside this module.
    #[must_use]
    pub(crate) fn into_variant(self, variant: usize, ctx: &PlanCtx) -> Arc<HlsVariant> {
        let Self {
            init,
            codec,
            container,
            segments,
        } = self;
        let init_size = init_size_of(&init);
        let layout = Layout::new(init_size, &segments);
        Arc::new(HlsVariant {
            variant,
            layout,
            cancel_epoch: CancelEpoch::new(ctx.master_cancel.clone()),
            profile: VariantProfile {
                codec,
                container,
                headers: ctx.headers.clone(),
                bus: ctx.bus.clone(),
            },
            segments: VariantSegments::new(init, segments),
            cache_complete_emitted: AtomicBool::new(false),
        })
    }
}

impl HlsVariant {
    pub(super) const NO_SEEK_TAIL: u32 = u32::MAX;

    pub(crate) fn event_bus(&self) -> EventBus {
        self.profile.bus.clone()
    }

    /// Index of the first non-`Loaded` segment — interpreted as the
    /// "download head" by the ABR controller. Returns `num_segments()`
    /// when every segment is `Loaded`. Scans linearly; cheap because it
    /// only runs from `Abr::progress` (ABR tick cadence).
    pub(crate) fn download_head(&self) -> u32 {
        let head = self
            .segments
            .iter()
            .position(|s| !s.state().is_loaded())
            .unwrap_or(self.segments.len());
        u32::try_from(head).unwrap_or(u32::MAX)
    }

    /// True when every fetch a `rebuild_queue(from_seg)` would enqueue is
    /// already `Loaded` — the init (if any) plus every media segment in
    /// `[from_seg, num_segments)`. In that state a rebuild only reseeds
    /// entries `dispatch` immediately skips, so it is pure churn on a
    /// fully-cached seek. A partial cache returns `false` and rebuilds, so
    /// the prefetch tail is still re-aimed at the seek target whenever a
    /// fetch is actually outstanding.
    pub(crate) fn fetch_plan_satisfied(&self, from_seg: u32) -> bool {
        if self.needs_init_fetch() {
            return false;
        }
        let segs_len = self.num_segments();
        (from_seg..segs_len).all(|idx| {
            self.segments
                .get(idx as usize)
                .is_some_and(|seg| seg.state().is_loaded())
        })
    }

    pub(crate) fn index(&self) -> usize {
        self.variant
    }

    pub(crate) fn is_encrypted_segment(&self, segment_index: u32) -> bool {
        self.segments
            .get(segment_index as usize)
            .is_some_and(|segment| matches!(segment.content(), SegmentContent::Encrypted(_)))
    }

    pub(crate) fn maybe_publish_cache_complete(&self) {
        if !self.fetch_plan_satisfied(0) {
            return;
        }
        if self.cache_complete_emitted.swap(true, Ordering::AcqRel) {
            return;
        }
        self.profile
            .bus
            .publish(kithara_events::HlsEvent::CacheComplete {
                total_bytes: self.authoritative_len(),
            });
    }

    pub(crate) fn variant_index_u32(&self) -> Option<u32> {
        u32::try_from(self.variant).ok()
    }

    /// Builds per-segment metadata. `#EXT-X-BYTERANGE` supplies an exact
    /// media-segment length when present; all other playlists get a non-exact
    /// routeable placeholder until body commit or lazy probe publishes the
    /// final length.
    fn build_segment_entries(
        playlist_state: &PlaylistState,
        decrypt_contexts: &[Option<DecryptContext>],
        variant_idx: usize,
        ctx: &PlanCtx,
    ) -> HlsResult<Vec<Segment>> {
        let scope = &ctx.scope;
        let Some(num) = playlist_state.num_segments(variant_idx) else {
            return Ok(Vec::new());
        };
        let bandwidth_bps = playlist_state.variant_bandwidth_bps(variant_idx);
        let mut decode_time = Duration::ZERO;
        let mut entries = Vec::with_capacity(num);
        for seg_idx in 0..num {
            let Some(url) = playlist_state.segment_url(variant_idx, seg_idx) else {
                break;
            };
            let duration = playlist_state
                .segment_decode_range(variant_idx, seg_idx)
                .map_or(Duration::ZERO, |(start, end)| end.saturating_sub(start));
            let decrypt_ctx = decrypt_contexts.get(seg_idx).cloned().flatten();
            let size = playlist_state
                .segment_byte_range_len(variant_idx, seg_idx)
                .map_or_else(
                    || SegmentSize::placeholder(segment_placeholder_size(duration, bandwidth_bps)),
                    SegmentSize::seed,
                );
            entries.push(Segment::Media(MediaSegment {
                resource_id: scope.key(&AssetResource::Url(url.clone()))?,
                url,
                state: SegmentSlotState::missing(),
                size,
                content: SegmentContent::from(decrypt_ctx),
                source: SegmentSource::new(ctx.scope.store().clone(), ctx.master_cancel.child()),
                decode_time,
                duration,
            }));
            decode_time = decode_time.saturating_add(duration);
        }
        Ok(entries)
    }
}
