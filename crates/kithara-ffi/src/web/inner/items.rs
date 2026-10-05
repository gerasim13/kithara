use kithara::platform::sync::Arc;

use super::WasmInner;
use crate::{item::AudioPlayerItem, types::FfiError, web::commands::WorkerCmd};
impl WasmInner {
    pub(crate) fn append(&self, item: &Arc<AudioPlayerItem>) -> Result<(), FfiError> {
        let id = item.track_id();
        self.try_send(WorkerCmd::Append {
            id,
            url: item.url(),
        })?;
        *item.inserted.lock() = true;
        self.queue_view.lock().push((id, Arc::clone(item)));
        item.restart_bridge();
        Ok(())
    }

    pub(crate) fn current_item(&self) -> Option<Arc<AudioPlayerItem>> {
        let current = self.bridge.current_track_id()?;
        self.queue_view
            .lock()
            .iter()
            .find(|(id, _)| *id == current)
            .map(|(_, item)| Arc::clone(item))
    }

    pub(crate) fn insert(
        &self,
        item: &Arc<AudioPlayerItem>,
        after: Option<&Arc<AudioPlayerItem>>,
    ) -> Result<(), FfiError> {
        let id = item.track_id();
        let after_id = after.map(|i| i.track_id());
        let request_id = Self::next_request_id();
        self.send(WorkerCmd::Insert {
            id,
            request_id,
            url: item.url(),
            after: after_id,
        });

        let mut view = self.queue_view.lock();
        let pos = match after_id {
            None => 0,
            Some(after_id) => view
                .iter()
                .position(|(existing, _)| *existing == after_id)
                .map(|i| i + 1)
                .ok_or_else(|| FfiError::InvalidArgument {
                    reason: format!("after id {after_id:?} not in queue"),
                })?,
        };
        view.insert(pos, (id, Arc::clone(item)));
        drop(view);

        *item.inserted.lock() = true;
        item.restart_bridge();
        Ok(())
    }

    pub(crate) fn item_count(&self) -> u32 {
        let len = self.queue_view.lock().len();
        u32::try_from(len).unwrap_or_else(|_| {
            tracing::error!(queue_len = len, "BUG: queue length exceeds u32::MAX");
            0
        })
    }

    pub(crate) fn items(&self) -> Vec<Arc<AudioPlayerItem>> {
        self.queue_view
            .lock()
            .iter()
            .map(|(_, item)| Arc::clone(item))
            .collect()
    }

    pub(crate) fn remove(&self, item: &AudioPlayerItem) -> Result<(), FfiError> {
        if !*item.inserted.lock() {
            return Err(FfiError::InvalidArgument {
                reason: format!("item {} not in queue", item.audio_id()),
            });
        }
        let id = item.track_id();
        let request_id = Self::next_request_id();
        self.send(WorkerCmd::Remove { id, request_id });
        self.queue_view
            .lock()
            .retain(|(existing, _)| *existing != id);
        *item.inserted.lock() = false;
        Ok(())
    }

    pub(crate) fn remove_all_items(&self) {
        self.send(WorkerCmd::RemoveAll);
        let mut view = self.queue_view.lock();
        for (_, item) in view.drain(..) {
            *item.inserted.lock() = false;
        }
    }

    pub(crate) fn replace_item(
        &self,
        index: u32,
        item: &Arc<AudioPlayerItem>,
    ) -> Result<(), FfiError> {
        let idx = index as usize;
        let new_id = item.track_id();
        let request_id = Self::next_request_id();

        let mut view = self.queue_view.lock();
        if idx >= view.len() {
            return Err(FfiError::InvalidArgument {
                reason: format!("item index {idx} out of range (len: {})", view.len()),
            });
        }
        self.send(WorkerCmd::Replace {
            index,
            request_id,
            id: new_id,
            url: item.url(),
        });
        if let Some((_, old)) = view.get(idx) {
            *old.inserted.lock() = false;
        }
        view[idx] = (new_id, Arc::clone(item));
        drop(view);

        *item.inserted.lock() = true;
        item.restart_bridge();
        Ok(())
    }

    pub(crate) fn select(
        &self,
        item: &AudioPlayerItem,
        transition: crate::types::FfiTransition,
    ) -> Result<(), FfiError> {
        if !*item.inserted.lock() {
            return Err(FfiError::InvalidArgument {
                reason: format!("item {} not in queue", item.audio_id()),
            });
        }
        let request_id = Self::next_request_id();
        self.send(WorkerCmd::SelectQueue {
            request_id,
            id: item.track_id(),
            transition: transition.try_into()?,
        });
        Ok(())
    }
}
