use kithara_platform::thread::spawn_named;

use super::ComputeSubmitError;
use crate::config::PoolConfig;

/// Owned compute pool that runs each admitted job on its own spawned thread.
pub(crate) struct ComputePool;

impl ComputePool {
    pub(super) const fn new() -> Self {
        Self
    }

    pub(super) fn spawner(&self, config: &PoolConfig) -> Result<Spawner, ComputeSubmitError> {
        let PoolConfig::OwnedLazy(config) = config else {
            return Err(ComputeSubmitError::Unavailable);
        };
        Ok(Spawner {
            name: config.name.clone(),
        })
    }
}

pub(super) struct Spawner {
    name: String,
}

impl Spawner {
    pub(super) fn spawn<F: FnOnce() + Send + 'static>(self, job: F) {
        drop(spawn_named(self.name, job));
    }
}
