use std::{fmt, ops::Range};

use kithara_assets::{AssetResourceState, AssetStore, AssetsError, ProcessCtx, ResourceKey};
use kithara_drm::{DecryptContext, as_process_ctx};
use kithara_file::{File, FileConfig, FileSrc};
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
    time::Duration,
};
use kithara_stream::{ReadOutcome, Source, SourceError, SourcePhase, StreamResult, StreamType};
use thiserror::Error;
use tracing::debug;
use url::Url;

use crate::segment::{SegmentSize, SegmentSlotState};

/// One `kithara-file` source over a segment resource — read-only when
/// attached to someone else's bytes, fetching when it owns them.
type FileHandle = <File as StreamType>::Source;

#[derive(Debug, Error)]
pub(crate) enum SegmentSourceError {
    #[error("failed to inspect attached file resource {resource:?}: {source}")]
    State {
        resource: ResourceKey,
        #[source]
        source: Box<AssetsError>,
    },
    #[error("failed to open attached file source for resource {resource:?}: {source}")]
    Open {
        resource: ResourceKey,
        #[source]
        source: Box<SourceError>,
    },
}

/// Shared home of the in-flight fetch. Exactly two parties reach it: the
/// slot that starts the fetch and the hook that ends it.
pub(crate) type FetchCell = Arc<Mutex<Option<FileHandle>>>;

pub(crate) struct SegmentSource {
    store: AssetStore,
    cancel: CancelToken,
    slot: Mutex<Option<FileHandle>>,
    /// The file fetching this slot's bytes, held for the fetch's lifetime
    /// only: the fetch owns the file, the slot merely keeps it alive.
    fetch: FetchCell,
}

impl SegmentSource {
    pub(crate) fn new(store: AssetStore, cancel: CancelToken) -> Self {
        Self {
            store,
            cancel,
            slot: Mutex::default(),
            fetch: FetchCell::default(),
        }
    }
}

/// Decryption disposition for a segment / init resource. Replaces
/// `decrypt_ctx: Option<DecryptContext>`, whose `.is_some()` /
/// `.map_or_else` discrimination on the acquire path conflated "no key"
/// with "cleartext". `Plain` is the explicit cleartext case; `Encrypted`
/// carries the AES-128 [`DecryptContext`].
#[derive(Debug, Clone)]
pub(crate) enum SegmentContent {
    Plain,
    Encrypted(DecryptContext),
}

impl From<Option<DecryptContext>> for SegmentContent {
    fn from(ctx: Option<DecryptContext>) -> Self {
        ctx.map_or(Self::Plain, Self::Encrypted)
    }
}

/// One cache slot in the variant's content domain. The shared-but-distinct
/// kinds — a separately fetched `#EXT-X-MAP` init prefix vs a media segment —
/// are folded under one enum (req 6): callers treat any slot uniformly through
/// the cascade methods (`state` / `size` / `read_at` / `contains`
/// / `len` / `url`), and the few media-only queries (`decode_time` / `duration`)
/// live on [`MediaSegment`].
///
/// Each cascade method dispatches DOWN into the arm, reproducing exactly the
/// per-kind code path the variant's `read_at` / `range_ready` /
/// `media_descriptor` / `init_descriptor_at` ran before the fold.
pub(crate) enum Segment {
    Init(InitSegment),
    Media(MediaSegment),
}

impl fmt::Debug for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(segment) => f
                .debug_struct("Segment::Init")
                .field("resource_id", &segment.resource_id)
                .field("url", &segment.url)
                .finish_non_exhaustive(),
            Self::Media(segment) => f
                .debug_struct("Segment::Media")
                .field("resource_id", &segment.resource_id)
                .field("url", &segment.url)
                .finish_non_exhaustive(),
        }
    }
}

impl Segment {
    /// The media arm, or `None` for the init slot — the seam for the
    /// media-only `decode_time` / `duration` accessors. `segments()` only ever
    /// holds `Media` arms, so callers iterating it always get `Some`.
    pub(crate) fn as_media(&self) -> Option<&MediaSegment> {
        match self {
            Self::Init(_) => None,
            Self::Media(s) => Some(s),
        }
    }

    /// Whether every byte in `range` is readable from this slot's file source.
    pub(crate) fn contains(&self, range: Range<u64>) -> bool {
        self.with_attached_source(|source| source.phase_at(range) == SourcePhase::Ready)
            .unwrap_or(false)
    }

    /// Decryption disposition — the fetch's processing context comes from it.
    pub(crate) fn content(&self) -> &SegmentContent {
        match self {
            Self::Init(s) => &s.content,
            Self::Media(s) => &s.content,
        }
    }

    /// Where the in-flight fetch lives while it runs.
    pub(crate) fn fetch_cell(&self) -> FetchCell {
        Arc::clone(&self.source().fetch)
    }

    /// The transform the resource commits through: decryption for an
    /// encrypted segment, nothing for a cleartext one.
    pub(crate) fn process(&self) -> Option<ProcessCtx> {
        match self.content() {
            SegmentContent::Plain => None,
            SegmentContent::Encrypted(context) => Some(as_process_ctx(context.clone())),
        }
    }

    delegate::delegate! {
        to self {
            /// Route byte length: `size().get()`. The route length is stable virtual
            /// geometry; the completeness gate is `size().is_exact()`.
            #[expr($.get())]
            #[call(size)]
            pub(crate) fn len(&self) -> u64;
            /// Read byte length: exact committed/probed length when known, otherwise
            /// the route length so descriptors remain addressable before commit.
            #[expr($.read_len())]
            #[call(size)]
            pub(crate) fn read_len(&self) -> u64;
        }
    }

    /// Wait for and read `range` through the slot's attached file source.
    /// `Ok(None)` means no source has been attached or the source is pending.
    pub(crate) fn read_at(&self, range: Range<u64>, dst: &mut [u8]) -> StreamResult<Option<usize>> {
        let Some(mut source) = self.with_attached_source(Clone::clone) else {
            return Ok(None);
        };
        let _ = source.wait_range(range.clone(), None)?;
        match source.read_at(range.start, dst)? {
            ReadOutcome::Bytes(count) => Ok(Some(count.get())),
            ReadOutcome::Pending(_) => Ok(None),
            ReadOutcome::Eof => Ok(Some(0)),
        }
    }

    /// Runs `use_source` under the source-slot mutex; the closure must not block.
    fn with_attached_source<T>(&self, use_source: impl FnOnce(&FileHandle) -> T) -> Option<T> {
        let source = self.source();
        let mut slot = source.slot.lock();
        if let Some(attached) = slot.as_ref() {
            return Some(use_source(attached));
        }
        let resource = self.resource_id().clone();
        let state = match source.store.resource_state(&resource) {
            Ok(state) => state,
            Err(error) => {
                let error = SegmentSourceError::State {
                    resource,
                    source: Box::new(error),
                };
                debug!(error = %error, "attached file source unavailable");
                return None;
            }
        };
        if !matches!(
            state,
            AssetResourceState::Active | AssetResourceState::Committed { .. }
        ) {
            return None;
        }
        let config = FileConfig::for_src(FileSrc::Attached(resource.clone()))
            .store(source.store.clone())
            .cancel(source.cancel.clone())
            .build();
        match File::open_attached(config) {
            Ok(attached) => {
                *slot = Some(attached);
                slot.as_ref().map(use_source)
            }
            Err(error) => {
                let error = SegmentSourceError::Open {
                    resource,
                    source: Box::new(error),
                };
                debug!(error = %error, "attached file source unavailable");
                None
            }
        }
    }

    pub(super) fn attached_len(&self) -> Option<u64> {
        self.with_attached_source(Source::len).flatten()
    }

    fn source(&self) -> &SegmentSource {
        match self {
            Self::Init(segment) => &segment.source,
            Self::Media(segment) => &segment.source,
        }
    }

    /// The segment's resource key.
    pub(crate) fn resource_id(&self) -> &ResourceKey {
        match self {
            Self::Init(s) => &s.resource_id,
            Self::Media(s) => &s.resource_id,
        }
    }

    /// The segment's byte-length atom (seed or committed). `len()` reads it.
    pub(crate) fn size(&self) -> &SegmentSize {
        match self {
            Self::Init(s) => &s.size,
            Self::Media(s) => &s.size,
        }
    }

    /// Cache state. Shared with the slot's `FetchSlot` handle (see
    /// [`MediaSegment::state`]).
    pub(crate) fn state(&self) -> &Arc<SegmentSlotState> {
        match self {
            Self::Init(s) => &s.state,
            Self::Media(s) => &s.state,
        }
    }

    /// The segment's fetch URL.
    pub(crate) fn url(&self) -> &Url {
        match self {
            Self::Init(s) => &s.url,
            Self::Media(s) => &s.url,
        }
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct MediaSegment {
    /// Cache state. The owning [`FetchClaim<Downloading>`](crate::segment::FetchClaim) handle (held by the
    /// segment's `FetchSlot`) shares this `Arc` and flips it on settle:
    /// `Loaded` on success, `Missing` on recoverable failure. Stale
    /// settles (cancelled before completion) are gated by
    /// `FetchSlot.cancel` and leave the slot untouched.
    pub(crate) state: Arc<SegmentSlotState>,
    #[field(get, vis = "pub(crate)", copy)]
    pub(crate) decode_time: Duration,
    #[field(get, vis = "pub(crate)", copy)]
    pub(crate) duration: Duration,
    pub(crate) resource_id: ResourceKey,
    pub(crate) content: SegmentContent,
    pub(crate) source: SegmentSource,
    /// Media size state. Non-exact placeholders are routeable before commit;
    /// committed lengths update the contiguous HLS byte map.
    pub(crate) size: SegmentSize,
    pub(crate) url: Url,
}

pub(crate) struct InitSegment {
    /// Shared with the init segment's `FetchSlot` handle; see
    /// [`MediaSegment::state`].
    pub(crate) state: Arc<SegmentSlotState>,
    pub(crate) resource_id: ResourceKey,
    pub(crate) source: SegmentSource,
    /// Decryption disposition for the init segment. HLS init segments
    /// don't carry their own `#EXT-X-KEY`; an encrypted variant mirrors
    /// the first media segment's key — the standard packaging convention.
    pub(crate) content: SegmentContent,
    /// Init size state — see [`MediaSegment::size`].
    pub(crate) size: SegmentSize,
    pub(crate) url: Url,
}
