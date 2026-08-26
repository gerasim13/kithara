use std::{
    io,
    io::Error,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use kithara_assets::WriterEpoch;
use kithara_events::{FileError, FileEvent, TotalBytesSource};
use kithara_net::{Headers, NetError, Retryability};
use kithara_platform::{
    CancelToken,
    sync::{Arc, Weak},
};
use kithara_stream::MediaInfo;

use crate::session::inner::FileInner;

pub(super) struct FetchWriter {
    pub(super) invalid_response: Arc<AtomicBool>,
    pub(super) offset: Arc<AtomicU64>,
    pub(super) cancel: CancelToken,
    pub(super) epoch: WriterEpoch,
    pub(super) inner: Weak<FileInner>,
}

impl FetchWriter {
    pub(super) fn write(&self, chunk: &[u8]) -> io::Result<()> {
        if self.invalid_response.load(Ordering::Acquire) {
            return Err(Error::other(
                "bounded response did not identify the requested range",
            ));
        }
        let chunk_len = u64::try_from(chunk.len()).map_err(|error| {
            Error::other(format!("file chunk length does not fit u64: {error}"))
        })?;
        let pos = self.offset.load(Ordering::Acquire);
        let end = pos
            .checked_add(chunk_len)
            .ok_or_else(|| Error::other("file download offset overflow"))?;
        match self.epoch.write_at(pos, chunk).current() {
            Some(Ok(())) => {
                self.offset.store(end, Ordering::Release);
                if let Some(inner) = self.inner.upgrade() {
                    inner.source.coord.set_download_pos(end);
                }
                Ok(())
            }
            Some(Err(error)) => Err(Error::other(error)),
            None => {
                self.cancel.cancel();
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct FetchCompletion<'a> {
    pub(super) end_exclusive: Option<u64>,
    pub(super) error: Option<&'a NetError>,
    pub(super) invalid_response: bool,
    pub(super) bytes_written: u64,
    pub(super) resume_from: u64,
}

impl FileInner {
    /// Seed size and media hints from a validated response.
    /// Ranges use `Content-Range`; an ignored initial range uses full-body length.
    pub(super) fn capture_content_metadata(
        &self,
        headers: &Headers,
        resume_from: u64,
        end_exclusive: Option<u64>,
    ) -> bool {
        let contract = response_contract(headers, resume_from, end_exclusive);
        if contract.invalid {
            return false;
        }
        let previous_total = self.source.coord.total_bytes();
        if previous_total
            .zip(contract.total)
            .is_some_and(|(previous, current)| previous != current)
        {
            return false;
        }
        if let Some(total_bytes) = contract.total {
            self.source.coord.set_total_bytes(Some(total_bytes));
            if previous_total != Some(total_bytes) {
                self.publish_total_bytes_resolved(total_bytes, TotalBytesSource::ContentLength);
            }
        }
        let info = headers.get("content-type").and_then(MediaInfo::parse_mime);
        if let Some(i) = info {
            let _ = self.content_type_info.set(i);
        }
        self.publish_opened(self.source.coord.total_bytes(), false, None);
        true
    }

    pub(super) fn complete_fetch(&self, epoch: &WriterEpoch, completion: FetchCompletion<'_>) {
        if completion.invalid_response && !matches!(completion.error, Some(NetError::Cancelled)) {
            self.fail_current_epoch(
                epoch,
                format!(
                    "ranged response did not identify the requested interval at offset {}",
                    completion.resume_from
                ),
            );
            return;
        }
        self.finalize_fetch(epoch, completion);
    }

    pub(super) fn fail_current_epoch(&self, epoch: &WriterEpoch, reason: String) {
        if let Some(result) = epoch.fail(reason.clone()).current() {
            let message = match result {
                Ok(()) => reason,
                Err(cleanup) => format!("{reason}; cleanup failed: {cleanup}"),
            };
            self.source.bus.publish(FileEvent::Error {
                error: FileError::Io(message),
            });
        }
    }

    /// Settle a fetch against its writer epoch and current byte coverage.
    /// Cancellation relinquishes; fatal or initial zero-progress errors fail.
    pub(super) fn finalize_fetch(&self, epoch: &WriterEpoch, completion: FetchCompletion<'_>) {
        if self.settle_fetch_error(epoch, completion) {
            return;
        }
        if !epoch.is_current() {
            return;
        }
        if let Some(final_len) = self.resolved_final_len(completion) {
            self.commit_if_complete(epoch, final_len);
        }
    }

    fn resolved_final_len(&self, completion: FetchCompletion<'_>) -> Option<u64> {
        if let Some(total) = self.source.coord.total_bytes() {
            return Some(total);
        }
        if completion.error.is_none()
            && completion.resume_from == 0
            && completion.end_exclusive.is_none()
        {
            return completion.resume_from.checked_add(completion.bytes_written);
        }
        None
    }

    fn settle_fetch_error(&self, epoch: &WriterEpoch, completion: FetchCompletion<'_>) -> bool {
        let Some(error) = completion.error else {
            return false;
        };
        if matches!(error, NetError::Cancelled) {
            let _ = epoch.relinquish();
            return true;
        }
        if error.retryability() == Retryability::Fatal
            || (completion.resume_from == 0 && completion.bytes_written == 0)
        {
            self.fail_current_epoch(epoch, error.to_string());
            return true;
        }
        false
    }
}

#[derive(Clone, Copy)]
pub(super) struct ResponseContract {
    pub(super) total: Option<u64>,
    pub(super) invalid: bool,
}

pub(super) fn response_contract(
    headers: &Headers,
    resume_from: u64,
    end_exclusive: Option<u64>,
) -> ResponseContract {
    let range_requested = end_exclusive.is_some() || resume_from > 0;
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(value) = headers.get("content-range") {
        let Some((response_start, response_end, total)) = parse_content_range(value) else {
            return ResponseContract {
                invalid: true,
                total: None,
            };
        };
        let span = response_end
            .checked_sub(response_start)
            .and_then(|span| span.checked_add(1));
        let invalid = response_start != resume_from
            || total.is_none_or(|total| response_end >= total)
            || end_exclusive.is_some_and(|requested_end| response_end >= requested_end)
            || content_length
                .zip(span)
                .is_none_or(|(content_length, span)| content_length != span);
        return ResponseContract {
            invalid,
            total: if invalid { None } else { total },
        };
    }

    if range_requested {
        let full_response = resume_from == 0 && content_length.is_some();
        return ResponseContract {
            invalid: !full_response,
            total: content_length.filter(|_| full_response),
        };
    }

    ResponseContract {
        invalid: false,
        total: content_length.and_then(|length| resume_from.checked_add(length)),
    }
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let (unit, value) = value.split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    if end < start {
        return None;
    }
    let total = if total == "*" {
        None
    } else {
        Some(total.parse::<u64>().ok()?)
    };
    Some((start, end, total))
}
