use std::sync::atomic::Ordering;

use kithara_test_utils::kithara;

use super::HlsVariant;

impl HlsVariant {
    pub(crate) fn prefetch_anchor(&self) -> u64 {
        self.flow.prefetch_anchor.load(Ordering::Acquire)
    }

    pub(crate) fn register_session_seek(&self, pos: u64, moved: bool) {
        if !self.flow.reader.is_seek_active() {
            self.retire_seek_projection_if_moved(pos);
        }
        if moved {
            self.set_exact_byte_seek_demand(pos);
        }
    }

    #[kithara::probe(variant = self.variant as u64, byte)]
    pub(crate) fn set_prefetch_anchor(&self, byte: u64) {
        self.flow.prefetch_anchor.store(byte, Ordering::Release);
    }
}
