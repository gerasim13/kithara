use kithara_assets::{AcquisitionResult, ReadSide, ResourceAcquisition, WriteSide};
use kithara_platform::{CancelToken, sync::Arc};
use kithara_storage::ResourceStatus;
use kithara_stream::dl::{FetchCmd, OnCompleteFn, OnSlowFn, WriterFn};
use url::Url;

use super::HlsVariant;
use crate::{
    segment::{Downloading, FetchClaim, FetchSlot},
    signal::SizeSignal,
};

impl HlsVariant {
    /// Builds a fetch command whose completion settles the claim under its cancellation epoch.
    pub(super) fn build_cmd(
        self: &Arc<Self>,
        url: Url,
        acq: ResourceAcquisition,
        handle: FetchClaim<Downloading>,
        signal: SizeSignal,
        cancel: CancelToken,
    ) -> Option<FetchCmd> {
        let writer = match acq {
            AcquisitionResult::Pending(writer) => writer,
            // WHY: A concurrent cache commit supplies the authoritative on-disk length.
            AcquisitionResult::Ready(reader) => {
                match reader.status() {
                    ResourceStatus::Committed { final_len: Some(n) } => {
                        handle.into_loaded(n);
                    }
                    _ => {
                        handle.into_loaded_no_apply();
                    }
                }
                signal.fire();
                return None;
            }
            _ => {
                handle.into_missing();
                return None;
            }
        };
        let slot = FetchSlot {
            handle,
            reader: writer.reader(),
            raw: writer.raw_write_handle(),
            writer,
            cancel: cancel.clone(),
            signal: signal.clone(),
            bus: self.profile.bus.clone(),
        };
        // WHY: The timeout hook needs the slot cell after the claim moves into `on_complete`.
        let slow_slot = slot.handle.slot_state();
        let slow_signal = signal.clone();
        let on_slow: OnSlowFn = Box::new(move || {
            slow_slot.mark_slow();
            // WHY: Stalled-escape reconciliation has no reader progress to wake it.
            slow_signal.wake_peer();
        });
        let mut inner_writer = slot.writer();
        // WHY: Readers and the audio worker need byte-arrival wakes before terminal settle.
        let writer_fn: WriterFn = Box::new(move |chunk: &[u8]| {
            let result = inner_writer(chunk);
            if result.is_ok() {
                signal.fire();
            }
            result
        });
        Some(
            FetchCmd::get(url)
                .cancel(cancel)
                .maybe_headers(self.profile.headers.clone())
                .writer(writer_fn)
                .on_slow(on_slow)
                .on_complete(OnCompleteFn::from(slot))
                .build(),
        )
    }
}
