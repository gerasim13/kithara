use std::{fmt, num::NonZeroUsize};

use bon::Builder;
use kithara_assets::AssetStore;
use kithara_bufpool::HasPool;
use kithara_platform::CancelToken;
use kithara_play::PlayerImpl;

/// Default parallelism cap for async track loads.
pub(crate) const DEFAULT_MAX_CONCURRENT_LOADS: NonZeroUsize = match NonZeroUsize::new(3) {
    Some(n) => n,
    None => unreachable!(),
};

/// Default prefetch lead time before EOF, in seconds.
///
/// Mirrors `kithara_play::PlayerConfig::prefetch_duration` default.
pub(crate) const DEFAULT_PREFETCH_DURATION: f32 = 3.5;

/// Configuration for a [`Queue`](crate::Queue).
///
/// Holds queue-level defaults plus the owned [`PlayerImpl`] instance whose
/// item list the queue coordinates.
///
/// [`TrackSource::Uri`](crate::TrackSource::Uri) resources share this queue's
/// store. A caller-supplied [`ResourceConfig`](kithara_play::ResourceConfig)
/// retains its own store.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct QueueConfig<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Max concurrent background prefetch loads. Default: 3.
    #[builder(default = DEFAULT_MAX_CONCURRENT_LOADS)]
    pub max_concurrent_loads: NonZeroUsize,

    /// Master cancel for the queue. `Some` threads the app master so the
    /// queue subtree cascades from one app-wide owner; `None` falls back
    /// to a fresh standalone token (test / library use). Must never be
    /// `None` on the production app path.
    pub cancel: Option<CancelToken>,

    /// Player owned and decorated by this queue.
    pub player: PlayerImpl<S>,

    /// Shared store used for bare URI track sources.
    pub store: Option<AssetStore<S>>,

    /// Whether the queue auto-advances to the next track at EOF.
    #[builder(default = true)]
    pub should_autoplay: bool,

    /// Lead time in seconds before EOF at which the next queued track
    /// is preloaded into the audio processor. Default: 3.5.
    #[builder(default = DEFAULT_PREFETCH_DURATION)]
    pub prefetch_duration: f32,

    /// Entries the navigation history keeps. Only explicit selections and
    /// auto-advances land there, so the default is a listening session's
    /// worth of back-steps; the queue's own track list is unbounded.
    #[builder(default = 100)]
    pub max_history_size: usize,
}

impl<S> fmt::Debug for QueueConfig<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueConfig")
            .field("max_concurrent_loads", &self.max_concurrent_loads)
            .field("should_autoplay", &self.should_autoplay)
            .field("prefetch_duration", &self.prefetch_duration)
            .field("max_history_size", &self.max_history_size)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_play::{PlayWorker, PlayWorkerConfig, PlayerConfig};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{queue::test_session, test_pools::pools};

    #[kithara::test]
    fn default_config_has_reasonable_loader_cap() {
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker)
                .session(test_session())
                .build(),
        );
        let cfg = QueueConfig::builder().player(player).build();
        assert_eq!(cfg.max_concurrent_loads.get(), 3);
        assert!(cfg.store.is_none());
        assert!((cfg.prefetch_duration - 3.5).abs() < f32::EPSILON);
    }
}
