mod response;

use std::{
    io,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::{Context, Poll},
};

use kithara_abr::Abr;
use kithara_assets::{ReadSide, ResourceLease, WriterEpoch, WriterHandle};
use kithara_net::{Headers, NetError, RangeSpec};
use kithara_platform::{
    CancelToken, CancelWakerGuard,
    sync::{Arc, Mutex, Weak},
};
use kithara_storage::ResourceStatus;
use kithara_stream::dl::{FetchCmd, Peer, RequestPriority, reject_html_response};

use self::response::{FetchCompletion, FetchWriter, response_contract};
use crate::session::inner::FileInner;

/// Gap-driven downloader for one remote file session.
/// It emits at most one fetch and waits when finite demand is already present.
pub(crate) struct FilePeer {
    _session_cancel_wake: CancelWakerGuard,
    _source_cancel_wake: CancelWakerGuard,
    /// Whether this peer has an in-flight fetch.
    inflight: Arc<AtomicBool>,
    inner: Weak<FileInner>,
    /// Current single-writer election handle, if this consumer owns it.
    writer: Mutex<Option<WriterHandle>>,
    session_cancel: CancelToken,
    source_cancel: CancelToken,
}

struct WriterSnapshot {
    cancel: CancelToken,
    epoch: WriterEpoch,
    watermark: u64,
}

struct FetchPlan {
    cancel: CancelToken,
    end_exclusive: Option<u64>,
    epoch: WriterEpoch,
    start: u64,
}

enum PeerAction {
    Done,
    Fetch(FetchPlan),
    Pending,
}

impl FilePeer {
    /// Build the remote session's download peer.
    ///
    /// # Panics
    ///
    /// Panics if `inner` has no resource lease. Local and already-cached files
    /// do not create peers.
    pub(crate) fn new(inner: &Arc<FileInner>, writer: Option<WriterHandle>) -> Self {
        let Some(lease) = inner.resource_lease.as_ref() else {
            panic!("BUG: FilePeer requires a resource lease");
        };
        let session_cancel = lease.session_cancel();
        let source_cancel = inner.source.cancel.clone();
        let source_cancel_wake = wake_peer_on_cancel(&inner.source.cancel, inner);
        let session_cancel_wake = wake_peer_on_cancel(&session_cancel, inner);
        Self {
            _session_cancel_wake: session_cancel_wake,
            _source_cancel_wake: source_cancel_wake,
            inflight: Arc::new(AtomicBool::new(false)),
            inner: Arc::downgrade(inner),
            writer: Mutex::new(writer),
            session_cancel,
            source_cancel,
        }
    }

    fn build_fetch_cmd(&self, inner: &Arc<FileInner>, plan: FetchPlan) -> FetchCmd {
        let FetchPlan {
            cancel: writer_cancel,
            end_exclusive,
            epoch,
            start,
        } = plan;
        let url = inner.asset.url.clone();
        let headers = inner.asset.headers.clone();
        let source_cancel = inner.source.cancel.clone();
        let fetch_cancel = writer_cancel.child();
        let cancel_from_source = fetch_cancel.clone();
        let source_cancel_guard = source_cancel.on_cancel(move || cancel_from_source.cancel());

        let invalid_response = Arc::new(AtomicBool::new(false));
        let offset = Arc::new(AtomicU64::new(start));
        let writer_state = FetchWriter {
            cancel: fetch_cancel.clone(),
            epoch: epoch.clone(),
            inner: Arc::downgrade(inner),
            invalid_response: Arc::clone(&invalid_response),
            offset: Arc::clone(&offset),
        };
        let writer = Box::new(move |chunk: &[u8]| -> io::Result<()> { writer_state.write(chunk) });

        let weak_for_response = Arc::downgrade(inner);
        let response_epoch = epoch.clone();
        let invalid_for_response = Arc::clone(&invalid_response);
        let on_response = Box::new(move |headers: &Headers| {
            if response_epoch.is_current() {
                let invalid = weak_for_response.upgrade().map_or_else(
                    || response_contract(headers, start, end_exclusive).invalid,
                    |inner| !inner.capture_content_metadata(headers, start, end_exclusive),
                );
                invalid_for_response.store(invalid, Ordering::Release);
            }
        });

        let weak_for_complete = Arc::downgrade(inner);
        let invalid_for_complete = Arc::clone(&invalid_response);
        let inflight = Arc::clone(&self.inflight);
        let cb_offset = Arc::clone(&offset);
        let on_complete = Box::new(
            move |_reported_total: u64, _headers: Option<&Headers>, err: Option<&NetError>| {
                drop(source_cancel_guard);
                let written = cb_offset.load(Ordering::Acquire).saturating_sub(start);
                if let Some(inner) = weak_for_complete.upgrade() {
                    inner.complete_fetch(
                        &epoch,
                        FetchCompletion {
                            bytes_written: written,
                            end_exclusive,
                            error: err,
                            invalid_response: invalid_for_complete.load(Ordering::Acquire),
                            resume_from: start,
                        },
                    );
                }
                inflight.store(false, Ordering::Release);
            },
        );

        FetchCmd::get(url)
            .cancel(fetch_cancel)
            .writer(writer)
            .validator(reject_html_response)
            .on_response(on_response)
            .maybe_range(fetch_range(start, end_exclusive))
            .maybe_headers(headers)
            .on_complete(on_complete)
            .build()
    }

    /// Snapshot the current writer without dropping election state under File's lock.
    fn writer_snapshot(&self, lease: &ResourceLease) -> Option<WriterSnapshot> {
        let stale = {
            let mut writer = self.writer.lock();
            if writer.as_ref().is_some_and(|handle| !handle.is_current()) {
                writer.take()
            } else {
                None
            }
        };
        drop(stale);

        let needs_writer = self.writer.lock().is_none();
        let candidate = if needs_writer {
            lease.try_take_writer()
        } else {
            None
        };
        let rejected = candidate.and_then(|candidate| {
            let mut writer = self.writer.lock();
            let rejected = if writer.is_none() {
                *writer = Some(candidate);
                None
            } else {
                Some(candidate)
            };
            drop(writer);
            rejected
        });
        drop(rejected);

        let (snapshot, stale) = {
            let mut writer = self.writer.lock();
            match writer.as_ref() {
                Some(handle) if handle.is_current() => (
                    Some(WriterSnapshot {
                        cancel: handle.writer_cancel(),
                        epoch: handle.epoch(),
                        watermark: handle.max_watermark(),
                    }),
                    None,
                ),
                Some(_) => (None, writer.take()),
                None => (None, None),
            }
        };
        drop(stale);
        snapshot
    }

    fn drop_writer(&self) {
        let writer = self.writer.lock().take();
        drop(writer);
    }

    fn next_action(&self, inner: &Arc<FileInner>, lease: &ResourceLease) -> PeerAction {
        if inner.source.cancel.is_cancelled() || self.session_cancel.is_cancelled() {
            self.drop_writer();
            return PeerAction::Done;
        }
        if inner.observe_committed() {
            return PeerAction::Done;
        }
        if !matches!(inner.asset.reader.status(), ResourceStatus::Active) {
            return PeerAction::Done;
        }

        let Some(writer) = self.writer_snapshot(lease) else {
            return PeerAction::Pending;
        };
        let total = inner.source.coord.total_bytes();
        let upper = total.map_or(writer.watermark, |total| total.min(writer.watermark));
        let Some(gap) = inner.asset.reader.next_gap(0, upper) else {
            return PeerAction::Pending;
        };
        let end_exclusive = (total.is_some() || writer.watermark != u64::MAX).then_some(gap.end);
        PeerAction::Fetch(FetchPlan {
            cancel: writer.cancel,
            end_exclusive,
            epoch: writer.epoch,
            start: gap.start,
        })
    }
}

fn wake_peer_on_cancel(cancel: &CancelToken, inner: &Arc<FileInner>) -> CancelWakerGuard {
    let weak = Arc::downgrade(inner);
    cancel.on_cancel(move || {
        if let Some(inner) = weak.upgrade()
            && let Some(lease) = inner.resource_lease.as_ref()
        {
            lease.wake_peer();
        }
    })
}

fn fetch_range(start: u64, end_exclusive: Option<u64>) -> Option<RangeSpec> {
    end_exclusive.map_or_else(
        || (start > 0).then(|| RangeSpec::new(start, None)),
        |end| {
            end.checked_sub(1)
                .filter(|end| *end >= start)
                .map(|end| RangeSpec::new(start, Some(end)))
        },
    )
}

impl Abr for FilePeer {
    fn cancel(&self) -> CancelToken {
        self.source_cancel.clone()
    }
}

impl Peer for FilePeer {
    fn poll_next(&self, cx: &mut Context<'_>) -> Poll<Option<Vec<FetchCmd>>> {
        if self.inflight.load(Ordering::Acquire) {
            return Poll::Pending;
        }
        let Some(inner) = self.inner.upgrade() else {
            return Poll::Ready(None);
        };
        let Some(lease) = inner.resource_lease.as_ref() else {
            return Poll::Ready(None);
        };

        lease.register_peer_waker(cx.waker());
        match self.next_action(&inner, lease) {
            PeerAction::Pending => Poll::Pending,
            PeerAction::Done => {
                lease.clear_peer_waker(cx.waker());
                Poll::Ready(None)
            }
            PeerAction::Fetch(plan) => {
                lease.clear_peer_waker(cx.waker());
                self.inflight.store(true, Ordering::Release);
                Poll::Ready(Some(vec![self.build_fetch_cmd(&inner, plan)]))
            }
        }
    }

    fn priority(&self) -> RequestPriority {
        if self
            .inner
            .upgrade()
            .is_some_and(|inner| inner.source.coord.activity().is_playing())
        {
            RequestPriority::High
        } else {
            RequestPriority::Low
        }
    }
}

#[cfg(test)]
mod tests;
