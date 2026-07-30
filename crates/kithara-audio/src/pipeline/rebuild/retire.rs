use std::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
};

use crossbeam_queue::ArrayQueue;
use tracing::warn;

use crate::pipeline::decode::DecoderGeneration;

pub(crate) struct RetiredGenerations {
    generations: ArrayQueue<DecoderGeneration>,
    overflowed: AtomicBool,
}

impl RetiredGenerations {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            generations: ArrayQueue::new(capacity),
            overflowed: AtomicBool::new(false),
        }
    }

    pub(crate) fn retire_generation(&self, generation: DecoderGeneration) {
        if let Err(generation) = self.generations.push(generation) {
            self.overflowed.store(true, Ordering::Release);
            mem::forget(generation);
        }
    }

    pub(crate) fn drain(&self) {
        while self.generations.pop().is_some() {}
        if self.overflowed.swap(false, Ordering::AcqRel) {
            warn!("decode retire queue overflowed; leaked retired state to keep RT core free");
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.generations.len()
    }
}
