use kithara_platform::thread::spawn_named;

use super::ComputeSubmitError;
use crate::config::PoolConfig;

/// Owned compute pool that runs each admitted job on its own spawned thread.
pub(crate) enum ComputePool {
    Disabled,
    Owned { name: String },
}

impl ComputePool {
    pub(super) fn new(config: PoolConfig) -> Self {
        match config {
            PoolConfig::Disabled => Self::Disabled,
            PoolConfig::OwnedLazy(config) => Self::Owned { name: config.name },
        }
    }

    pub(super) const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    pub(super) fn spawner(&self) -> Result<Spawner, ComputeSubmitError> {
        let Self::Owned { name } = self else {
            return Err(ComputeSubmitError::Unavailable);
        };
        Ok(Spawner { name: name.clone() })
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
