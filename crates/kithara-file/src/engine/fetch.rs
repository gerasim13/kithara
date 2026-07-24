use std::{
    io,
    io::Error,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use kithara_assets::{ProducerHandle, ReadSide};
use kithara_net::{Headers, NetError, RangeSpec};
use kithara_platform::sync::{Arc, Mutex};
use kithara_storage::ResourceStatus;
use kithara_stream::{
    MediaInfo,
    dl::{FetchCmd, reject_html_response},
};
use kithara_test_utils::kithara;
use url::Url;

use super::ResourceEngine;

impl ResourceEngine {
    #[kithara::probe(resume_from)]
    pub(crate) fn build_fetch_cmd<P, T>(
        self: &Arc<Self>,
        url: Url,
        headers: Option<Headers>,
        resume_from: u64,
        inflight: Arc<AtomicBool>,
        on_progress: P,
        total_bytes: T,
    ) -> FetchCmd
    where
        P: Fn(u64) + Send + Sync + 'static,
        T: Fn() -> Option<u64> + Send + Sync + 'static,
    {
        let cancel = self.identity.cancel.clone();

        let raw = self.raw.clone();
        let engine_for_write = Arc::clone(self);
        let offset = Arc::new(AtomicU64::new(resume_from));
        let writer_offset = Arc::clone(&offset);
        let writer = Box::new(move |chunk: &[u8]| -> io::Result<()> {
            let chunk_len = u64::try_from(chunk.len()).map_err(|err| {
                Error::other(format!("file chunk length does not fit u64: {err}"))
            })?;
            let pos = writer_offset.fetch_add(chunk_len, Ordering::Relaxed);
            let end = pos
                .checked_add(chunk_len)
                .ok_or_else(|| Error::other("file download offset overflow"))?;
            let Some(raw) = raw.as_ref() else {
                return Err(Error::other(
                    "file resource has no writer (already committed or read-only)",
                ));
            };
            raw.write_at(pos, chunk).map_err(Error::other)?;
            on_progress(end);
            engine_for_write.wake_worker();
            Ok(())
        });

        let engine_for_resp = Arc::clone(self);
        let on_response = Box::new(move |headers: &Headers| {
            engine_for_resp.capture_content_metadata(headers, resume_from);
        });

        let engine = Arc::clone(self);
        let cb_offset = Arc::clone(&offset);
        let on_complete = Box::new(
            move |_reported_total: u64, _headers: Option<&Headers>, err: Option<&NetError>| {
                let written = cb_offset
                    .load(Ordering::Relaxed)
                    .saturating_sub(resume_from);
                engine.finalize_fetch(resume_from, written, total_bytes(), err);
                inflight.store(false, Ordering::Release);
            },
        );

        FetchCmd::get(url)
            .cancel(cancel)
            .writer(writer)
            .validator(reject_html_response)
            .on_response(on_response)
            .maybe_range((resume_from > 0).then(|| RangeSpec::new(resume_from, None)))
            .maybe_headers(headers)
            .on_complete(on_complete)
            .build()
    }

    /// Pull `Content-Length` and `Content-Type` out of the response
    /// headers and seed the lifecycle sink. Both lookups try the
    /// lower-cased header first (per HTTP/2 RFC) and fall back to the
    /// title-cased form from older HTTP/1.1 servers.
    ///
    /// `resume_from` is the byte offset our Range request started at:
    /// on a `206 Partial Content` response, `Content-Length` describes
    /// the partial body length, so the resource's full size is
    /// `resume_from + content_length`.
    pub(crate) fn capture_content_metadata(&self, headers: &Headers, resume_from: u64) {
        let content_length = headers
            .get("content-length")
            .or_else(|| headers.get("Content-Length"))
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(len) = content_length {
            self.sink.total_bytes_resolved(resume_from + len);
        }
        let info = headers
            .get("content-type")
            .or_else(|| headers.get("Content-Type"))
            .and_then(MediaInfo::parse_mime);
        self.sink.metadata_resolved(info);
    }

    /// Whether this engine may issue GETs for the shared resource.
    ///
    /// Resources with no demand lease (standalone store, single
    /// consumer) always drive. With a lease, only the elected producer
    /// drives; a non-producer first tries to take over an abandoned slot
    /// (`try_take_producer`) and otherwise yields to the live producer.
    pub(crate) fn ensure_producer(&self, producer: &Mutex<Option<ProducerHandle>>) -> bool {
        let Some(lease) = self.demand_lease.as_ref() else {
            return true;
        };
        let mut producer = producer.lock();
        if producer.is_some() {
            return true;
        }
        if let Some(handle) = lease.try_take_producer() {
            *producer = Some(handle);
            return true;
        }
        drop(producer);
        false
    }

    /// Start of the next byte range worth fetching, or `None` when the
    /// resource is terminal (neither `Active` nor `Committed`) or already
    /// fully covered. The gap walk only runs once the status check passes.
    pub(crate) fn next_fetchable_gap(&self, total_bytes: Option<u64>) -> Option<u64> {
        matches!(
            self.reader.status(),
            ResourceStatus::Active | ResourceStatus::Committed { .. }
        )
        .then(|| self.next_gap_start(total_bytes))
        .flatten()
    }

    /// Start of the next missing byte range on this resource, or
    /// `None` when the resource is fully covered. Upper bound is the
    /// known total — committed length (when reactivating a partial)
    /// or the discovered `Content-Length` (after the first response
    /// headers seed the lifecycle sink). Without either, falls back
    /// to `u64::MAX` so the gap walker scans the whole space.
    pub(crate) fn next_gap_start(&self, total_bytes: Option<u64>) -> Option<u64> {
        let upper = self.reader.len().or(total_bytes).unwrap_or(u64::MAX);
        self.reader.next_gap(0, upper).map(|gap| gap.start)
    }
}
