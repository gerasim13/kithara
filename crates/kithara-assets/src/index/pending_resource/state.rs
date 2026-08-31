#![forbid(unsafe_code)]

use std::task::Waker;

use kithara_bufpool::HasPool;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
};

use super::{PendingResourceCleanupError, RemoveResource};
use crate::{
    index::pending::DemandEntry,
    resource::WriteSide,
    store::{AssetReader, AssetWriter},
};

#[derive(Debug)]
pub(crate) enum SessionPhase {
    Active,
    Finishing,
    Committed,
    CleanupFailed(PendingResourceCleanupError),
}

pub(in crate::index) struct WriterClaim {
    consumer: Arc<DemandEntry>,
}

impl WriterClaim {
    pub(super) fn belongs_to(&self, consumer: &Arc<DemandEntry>) -> bool {
        Arc::ptr_eq(&self.consumer, consumer)
    }
}

pub(crate) struct DemandState<S> {
    pub(crate) reader: Option<AssetReader<S>>,
    pub(crate) phase: SessionPhase,
    pub(crate) entries: Vec<Arc<DemandEntry>>,
    pub(super) writer_claim: Option<Arc<WriterClaim>>,
    pub(super) writer: Option<AssetWriter<S>>,
}

impl<S> DemandState<S> {
    pub(crate) fn current_peer_waker(&self) -> Option<Waker> {
        self.writer_claim
            .as_ref()
            .and_then(|claim| claim.consumer.take_peer_waker())
    }

    fn elect_writer(&mut self, consumer: &Arc<DemandEntry>) -> Option<Arc<WriterClaim>> {
        if !matches!(&self.phase, SessionPhase::Active) || self.writer_claim.is_some() {
            return None;
        }
        let claim = Arc::new(WriterClaim {
            consumer: Arc::clone(consumer),
        });
        self.writer_claim = Some(Arc::clone(&claim));
        Some(claim)
    }

    pub(super) fn is_current_writer(&self, claim: &Arc<WriterClaim>) -> bool {
        matches!(&self.phase, SessionPhase::Active)
            && self
                .writer_claim
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, claim))
    }

    pub(super) fn max_watermark(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| entry.watermark())
            .max()
            .unwrap_or(0)
    }

    pub(super) fn peer_wakers(&self) -> Vec<Waker> {
        self.entries
            .iter()
            .filter_map(|entry| entry.take_peer_waker())
            .collect()
    }

    pub(super) fn reader_wakers(&self) -> Vec<Waker> {
        self.entries
            .iter()
            .filter_map(|entry| entry.take_reader_waker())
            .collect()
    }

    pub(super) fn terminal_wakers(&self) -> Vec<Waker> {
        let mut wakers = self.reader_wakers();
        wakers.extend(self.peer_wakers());
        wakers
    }
}

pub(crate) struct PendingResource<S> {
    pub(crate) state: Mutex<DemandState<S>>,
    pub(super) writer_cancel: CancelToken,
    pub(super) remove: RemoveResource,
}

impl<S> PendingResource<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(crate) fn new(
        writer_cancel: CancelToken,
        consumer: Arc<DemandEntry>,
        writer: AssetWriter<S>,
        remove: RemoveResource,
    ) -> Self {
        let reader = writer.reader();
        Self {
            writer_cancel,
            remove,
            state: Mutex::new(DemandState {
                entries: vec![consumer],
                phase: SessionPhase::Active,
                writer_claim: None,
                reader: Some(reader),
                writer: Some(writer),
            }),
        }
    }

    pub(in crate::index) fn elect_writer(
        &self,
        state: &mut DemandState<S>,
        consumer: &Arc<DemandEntry>,
    ) -> Option<Arc<WriterClaim>> {
        if self.writer_cancel.is_cancelled() {
            return None;
        }
        state.elect_writer(consumer)
    }
}
