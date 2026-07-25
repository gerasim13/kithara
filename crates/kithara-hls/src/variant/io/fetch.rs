use std::sync::Weak;

use kithara_events::EventBus;
use kithara_file::{FetchCompleteFn, File, FileConfig, FileSrc, SlowFn};
use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::Activity;
use tracing::debug;

use super::HlsVariant;
use crate::{
    segment::{Downloading, FetchCell, FetchClaim, PlannedFetch, Segment, SegmentSettle},
    signal::SizeSignal,
    variant::PlanCtx,
};

impl HlsVariant {
    /// Fetch one segment / init slot as an ordinary file.
    ///
    /// kithara-file owns everything about the resource — acquire, the
    /// writer, resume, commit, and adopting a commit a racing writer made.
    /// This owns only what is timeline: which slot the bytes belong to and
    /// what its claim becomes when the fetch ends.
    ///
    /// The file lives for the fetch, not for the slot: coverage lives in
    /// the resource, so a later re-fetch resumes from whatever this one
    /// reached.
    pub(super) fn start_fetch(
        self: &Arc<Self>,
        ctx: &PlanCtx,
        segment: &Segment,
        handle: FetchClaim<Downloading>,
        signal: SizeSignal,
    ) -> Result<(), kithara_stream::SourceError> {
        let handle_planned = handle.planned();
        let claim = Arc::new(Mutex::new(Some(handle)));
        let cell = segment.fetch_cell();
        let config = FileConfig::for_src(FileSrc::Resource {
            key: segment.resource_id().clone(),
            url: segment.url().clone(),
        })
        // A segment fetch IS the track's network activity, so its transport
        // events belong on the track's bus — that is where a consumer counts
        // downloads and where ABR's throughput samples surface.
        .events(self.profile.bus.clone())
        .store(ctx.scope.store().clone())
        .downloader(ctx.downloader.clone())
        .cancel(self.cancel_handle())
        .maybe_headers(self.profile.headers.clone())
        .maybe_process(segment.process())
        .parent(ctx.parent.clone())
        .activity(Arc::new(SlotDemand {
            segment: handle_planned,
            variant: Arc::downgrade(self),
        }) as Arc<dyn Activity>)
        .on_fetch_complete(settle_hook(
            Arc::clone(&claim),
            Arc::clone(&cell),
            signal.clone(),
            self.profile.bus.clone(),
        ))
        .on_slow(slow_hook(Arc::clone(&claim), signal))
        .build();

        let file = match File::open_fetch(config) {
            Ok(file) => file,
            Err(error) => {
                // Hand the slot back rather than let the claim's Drop revert
                // it with a leaked-handle warning: nothing leaked, the fetch
                // simply never started.
                if let Some(handle) = claim.lock().take() {
                    handle.into_missing();
                }
                return Err(error);
            }
        };
        if !hold_fetch(&cell, &claim, file) {
            debug!(target: "kithara_hls::settle", "fetch settled during open — nothing to hold");
        }
        Ok(())
    }
}

/// Answers whether this slot is the one a reader is waiting on, so the
/// downloader ranks it ahead of the slots fetched after it.
///
/// Read live by the queue, so a segment fetched ahead of need is promoted
/// the moment the reader arrives in it. The init prefix always counts:
/// nothing in the variant decodes without it.
struct SlotDemand {
    segment: PlannedFetch,
    variant: Weak<HlsVariant>,
}

impl Activity for SlotDemand {
    fn is_playing(&self) -> bool {
        let Some(variant) = self.variant.upgrade() else {
            return false;
        };
        let PlannedFetch::Segment(index) = self.segment else {
            return true;
        };
        variant
            .find_at_offset(variant.get_position())
            .is_some_and(|(reader_segment, _, _)| reader_segment == index)
    }

    fn set_playing(&self, _playing: bool) {}
}

/// Park the fetching file for the slot to keep alive, reporting whether it
/// was still wanted — a committed resource needs no download, so the
/// completion hook can already have fired inside `open_fetch`.
///
/// The cell is locked *before* the claim is read, and the hook takes them the
/// other way round with the claim released first: whichever party reaches the
/// cell first decides, and the file is dropped exactly once.
fn hold_fetch(
    cell: &FetchCell,
    claim: &Mutex<Option<FetchClaim<Downloading>>>,
    file: <File as kithara_stream::StreamType>::Source,
) -> bool {
    let mut slot = cell.lock();
    if claim.lock().is_none() {
        return false;
    }
    *slot = Some(file);
    true
}

fn settle_hook(
    claim: Arc<Mutex<Option<FetchClaim<Downloading>>>>,
    cell: FetchCell,
    signal: SizeSignal,
    bus: EventBus,
) -> FetchCompleteFn {
    Arc::new(move |outcome| {
        SegmentSettle::new(&claim, &cell, &signal, &bus).apply(outcome);
    })
}

/// Flag the in-flight fetch slow so the ABR stalled-escape gate can see it,
/// and wake the peer's `poll_next` so `reconcile_escape` runs against the
/// now-stalled slot rather than waiting for an incidental reader wake.
fn slow_hook(claim: Arc<Mutex<Option<FetchClaim<Downloading>>>>, signal: SizeSignal) -> SlowFn {
    Arc::new(move || {
        if let Some(handle) = claim.lock().as_ref() {
            handle.slot_state().mark_slow();
        }
        signal.wake_peer();
    })
}
