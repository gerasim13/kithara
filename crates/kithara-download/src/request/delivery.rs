use kithara_abr::{AbrController, AbrPeerId, BandwidthSource};
use kithara_events::EventBus;
use kithara_net::{NetError, Retryability};
use kithara_platform::{
    CancelToken,
    sync::Arc,
    time::{Duration, WallInstant},
};
use kithara_test_utils::kithara;

use crate::{
    CancelReason, DownloaderEvent, RequestId,
    cmd::{FetchCmd, OnCompleteFn, OnResponseFn, WriterFn},
    peer::ResponseTarget,
    response::FetchResponse,
};

pub(crate) fn publish_cancelled(
    bus: Option<&EventBus>,
    request_id: RequestId,
    reason: CancelReason,
    bytes_transferred: u64,
) {
    if let Some(bus) = bus {
        bus.publish(DownloaderEvent::RequestCancelled {
            request_id,
            reason,
            bytes_transferred,
        });
    }
}

/// Compute pre-rounded bandwidth in bps. Guards against zero duration
/// (cache hits / instant responses) so subscribers don't repeat the
/// math or hit a div-by-zero.
fn bandwidth_bps(bytes: u64, duration: Duration) -> u64 {
    /// Bits per byte × milliseconds-per-second-power-of-ten conversion
    /// for the bps formula `bytes * 8 * 1000 / duration_ms`.
    const BITS_TIMES_MS_PER_SEC: u64 = 8_000;
    let ms = u64::try_from(duration.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    bytes.saturating_mul(BITS_TIMES_MS_PER_SEC) / ms
}

#[derive(Clone, Copy)]
pub(crate) struct Cancellation<'a> {
    pub(crate) downloader: &'a CancelToken,
    pub(crate) peer: &'a CancelToken,
    pub(crate) epoch: Option<&'a CancelToken>,
}

impl Cancellation<'_> {
    /// Determine why a fetch was cancelled.
    ///
    /// Order of checks reflects priority: peer-cancel implies the whole
    /// peer is going away; epoch-cancel is a per-fetch invalidation;
    /// downloader-shutdown is the global stop. `BeforeStart` catches the
    /// race where the cancel token was set before any fetch task ran.
    pub(crate) fn reason(self) -> CancelReason {
        if self.peer.is_cancelled() {
            CancelReason::PeerCancel
        } else if self.epoch.is_some_and(CancelToken::is_cancelled) {
            CancelReason::EpochCancel
        } else if self.downloader.is_cancelled() {
            CancelReason::DownloaderShutdown
        } else {
            CancelReason::BeforeStart
        }
    }
}

/// All the per-fetch context `deliver` needs: identity (request id, peer id,
/// abr controller), the fetch start (`started`), the body sinks (writer +
/// completion callback), the `bus` for telemetry, and the three nested cancel
/// tokens (peer, epoch, downloader) used to classify cancellation reasons.
pub(crate) struct DeliveryContext<'a> {
    pub(crate) cancellation: Cancellation<'a>,
    pub(crate) peer_id: AbrPeerId,
    pub(crate) abr: Arc<AbrController>,
    pub(crate) started: WallInstant,
    pub(crate) bus: Option<EventBus>,
    pub(crate) on_complete_cb: Option<OnCompleteFn>,
    pub(crate) on_response_cb: Option<OnResponseFn>,
    pub(crate) writer: Option<WriterFn>,
    pub(crate) target: ResponseTarget,
    pub(crate) result: Result<FetchResponse, NetError>,
}

/// Route a fetch result to its target and publish the matching
/// `DownloaderEvent` on `bus` (if any).
///
/// Collects the body on the downloader's possibly-separate worker so only `Send` bytes cross back
/// to the caller; the raw HTTP body stream is `!Send` on wasm.
#[kithara::probe(request_id)]
pub(crate) async fn deliver(request_id: RequestId, ctx: DeliveryContext<'_>) {
    let DeliveryContext {
        target,
        result,
        writer,
        on_complete_cb,
        on_response_cb,
        abr,
        peer_id,
        started,
        bus,
        cancellation,
    } = ctx;
    match target {
        ResponseTarget::Channel(tx) => {
            let collected = match result {
                Ok(resp) => {
                    let headers = resp.headers.clone();
                    resp.body.collect().await.map(|bytes| (headers, bytes))
                }
                Err(e) => Err(e),
            };
            tx.send(collected).ok();
        }
        ResponseTarget::Streaming => match result {
            Ok(resp) => {
                let headers = resp.headers.clone();
                let Some(mut w) = writer else {
                    if let Some(cb) = on_complete_cb {
                        cb(0, Some(&headers), None);
                    }
                    return;
                };
                if let Some(cb) = on_response_cb {
                    cb(&headers);
                }
                let write_result = resp.body.write_all(|chunk| w(chunk)).await;
                let elapsed = started.elapsed();
                match write_result {
                    Ok(total) => {
                        kithara::probe_event!(
                            finish_request,
                            request_id,
                            bytes_transferred = total,
                            duration = elapsed
                        );
                        if total > 0 {
                            abr.record_bandwidth(peer_id, total, elapsed, BandwidthSource::Network);
                        }
                        if let Some(bus) = bus.as_ref() {
                            bus.publish(DownloaderEvent::RequestCompleted {
                                request_id,
                                bytes_transferred: total,
                                duration: elapsed,
                                bandwidth_bps: bandwidth_bps(total, elapsed),
                            });
                        }
                        if let Some(cb) = on_complete_cb {
                            cb(total, Some(&headers), None);
                        }
                    }
                    Err(ref e) => {
                        publish_failure_or_cancel(bus.as_ref(), request_id, e, cancellation);
                        if let Some(cb) = on_complete_cb {
                            cb(0, Some(&headers), Some(e));
                        }
                    }
                }
            }
            Err(ref e) => {
                publish_failure_or_cancel(bus.as_ref(), request_id, e, cancellation);
                if let Some(cb) = on_complete_cb {
                    cb(0, None, Some(e));
                }
            }
        },
    }
}

/// Publish `RequestFailed` (network error) or `RequestCancelled`
/// (cancel token fired) depending on the error variant.
fn publish_failure_or_cancel(
    bus: Option<&EventBus>,
    request_id: RequestId,
    err: &NetError,
    cancellation: Cancellation<'_>,
) {
    if matches!(err, NetError::Cancelled) {
        let reason = cancellation.reason();
        let bytes_transferred = 0_u64;
        kithara::probe_event!(
            abort_request,
            request_id,
            reason,
            bytes_transferred,
            was_in_flight = true
        );
        publish_cancelled(bus, request_id, reason, bytes_transferred);
    } else {
        let retryable = err.retryability() == Retryability::Transient;
        kithara::probe_event!(fail_request, request_id, retryable);
        if let Some(bus) = bus {
            bus.publish(DownloaderEvent::RequestFailed {
                request_id,
                retryable,
                error: err.clone(),
            });
        }
    }
}

/// Route a cancellation to its target. Does NOT publish events — use
/// [`crate::batch::deliver_cancelled_with_event`] for that.
///
/// The oneshot is the channel path's completion signal, matching `deliver`,
/// which never calls `on_complete` for a `Channel` target. Only the streaming
/// path owns a claim through its callback.
pub(crate) fn deliver_cancelled(target: ResponseTarget, mut cmd: FetchCmd) {
    let err = NetError::Cancelled;
    match target {
        ResponseTarget::Channel(tx) => {
            tx.send(Err(err)).ok();
        }
        ResponseTarget::Streaming => {
            if let Some(cb) = cmd.take_on_complete() {
                cb(0, None, Some(&err));
            }
        }
    }
}
