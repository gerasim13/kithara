#![forbid(unsafe_code)]

#[cfg(target_arch = "wasm32")]
use std::sync::atomic::Ordering;
use std::sync::{OnceLock, atomic::AtomicBool};

use kithara_platform::sync::{Arc, Mutex};

use super::state::LruState;
use crate::{
    error::AssetsResult,
    index::persistence::{FlushHub, Flushable},
};

pub(super) struct LruInner {
    pub(super) dirty: AtomicBool,
    pub(super) state: Mutex<LruState>,
    /// Set by [`super::core::LruIndex::attach_to`]. While `None`, mutators flush
    /// inline through [`Flushable::flush`] — matches the historical
    /// inline-flush behaviour for ad-hoc tests.
    pub(super) hub: OnceLock<Arc<FlushHub>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) persist: Option<super::disk::LruPersist>,
}

impl Flushable for LruInner {
    fn dirty(&self) -> &AtomicBool {
        &self.dirty
    }

    fn flush(&self) -> AssetsResult<()> {
        self.flush_with_durability(false)
    }

    fn flush_durable(&self) -> AssetsResult<()> {
        self.flush_with_durability(true)
    }

    fn name(&self) -> &'static str {
        "lru"
    }
}

#[cfg(target_arch = "wasm32")]
impl LruInner {
    fn flush_with_durability(&self, _durable: bool) -> AssetsResult<()> {
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }
}
