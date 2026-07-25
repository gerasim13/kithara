use std::{
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use kithara_abr::Abr;
use kithara_assets::ProducerHandle;
use kithara_net::Headers;
use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::dl::{FetchCmd, Peer, PeerRef, RequestPriority};
use url::Url;

use super::FileSourceCtx;
use crate::engine::{FilePhase, ResourceEngine};

/// File-track Peer: thin downloader adapter over one resource engine.
/// Owns the remote origin — a local/cached source never constructs one.
pub(crate) struct FilePeer {
    /// `true` while a fetch issued by this peer is still in flight.
    /// Cleared from the `on_complete` callback before the Downloader
    /// re-polls.
    inflight: Arc<AtomicBool>,
    engine: Arc<ResourceEngine>,
    source: Arc<FileSourceCtx>,
    url: Url,
    headers: Option<Headers>,
    /// The peer this one fetches for, when it carries one piece of a larger
    /// download. `None` when the file is its own consumer.
    parent: Option<PeerRef>,
    /// Single-producer election handle. `Some` only while this peer is
    /// the elected producer for the shared resource. Starts as the
    /// attach-time winner (or `None` for a loser); a loser promotes
    /// itself once the previous producer drops. When no demand lease
    /// was attached the peer always drives.
    producer: Mutex<Option<ProducerHandle>>,
}

impl FilePeer {
    pub(crate) fn new(
        engine: Arc<ResourceEngine>,
        source: Arc<FileSourceCtx>,
        producer: Option<ProducerHandle>,
        url: Url,
        headers: Option<Headers>,
        parent: Option<PeerRef>,
    ) -> Self {
        Self {
            engine,
            source,
            inflight: Arc::new(AtomicBool::new(false)),
            producer: Mutex::new(producer),
            url,
            headers,
            parent,
        }
    }
}

impl Abr for FilePeer {}

impl Peer for FilePeer {
    fn poll_next(&self, _cx: &mut Context<'_>) -> Poll<Option<Vec<FetchCmd>>> {
        if self.inflight.load(Ordering::Acquire) {
            return Poll::Pending;
        }
        let Some(gap_start) = self
            .engine
            .next_fetchable_gap(self.source.coord.total_bytes())
        else {
            return Poll::Ready(None);
        };
        // A gap remains. Only the elected producer issues GETs so two
        // consumers of one shared resource (e.g. player + waveform) share
        // a single download. A non-producer yields and re-polls on the
        // next downloader tick (a sibling fetch completing), which also
        // covers producer handoff after the original producer drops.
        if !self.engine.ensure_producer(&self.producer) {
            return Poll::Pending;
        }
        self.inflight.store(true, Ordering::Release);
        self.engine.set_phase(FilePhase::Downloading, None);

        let progress = Arc::clone(&self.source.coord);
        let total = Arc::clone(&self.source.coord);
        let command = self.engine.build_fetch_cmd(
            self.url.clone(),
            self.headers.clone(),
            gap_start,
            Arc::clone(&self.inflight),
            move |end| progress.set_download_pos(end),
            move || total.total_bytes(),
        );
        Poll::Ready(Some(vec![command]))
    }

    fn parent(&self) -> Option<PeerRef> {
        self.parent.clone()
    }

    fn priority(&self) -> RequestPriority {
        if self.source.coord.activity().is_playing() {
            RequestPriority::HIGH
        } else {
            RequestPriority::LOW
        }
    }
}
