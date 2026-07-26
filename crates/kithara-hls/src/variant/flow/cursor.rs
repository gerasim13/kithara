use std::sync::atomic::Ordering;

use kithara_test_utils::kithara;

use super::ReadLease;

impl ReadLease {
    #[kithara::probe(variant = self.variant.index() as u64, n)]
    pub(crate) fn advance(&self, n: u64) {
        self.cursor.advance(n);
        if !self.cursor.is_seek_active() {
            self.clear_seek_alias_if_moved(self.cursor.position());
        }
    }

    #[kithara::probe(variant = self.variant.index() as u64, pos = self.cursor.position())]
    pub(crate) fn get_position(&self) -> u64 {
        self.cursor.position()
    }

    pub(crate) fn prefetch_anchor(&self) -> u64 {
        self.prefetch_anchor.load(Ordering::Acquire)
    }

    #[kithara::probe(variant = self.variant.index() as u64, pos)]
    pub(crate) fn set_position(&self, pos: u64) {
        let moved = self.cursor.position() != pos;
        self.set_position_without_byte_demand(pos);
        if moved {
            self.set_exact_byte_seek_demand(pos);
        }
    }

    pub(super) fn set_position_without_byte_demand(&self, pos: u64) {
        if !self.cursor.is_seek_active() {
            self.clear_seek_alias_if_moved(pos);
        }
        self.cursor.set_position(pos);
    }

    #[kithara::probe(variant = self.variant.index() as u64, byte)]
    pub(crate) fn set_prefetch_anchor(&self, byte: u64) {
        self.prefetch_anchor.store(byte, Ordering::Release);
    }
}
