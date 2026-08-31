use kithara::bufpool::{OverallBudget, Percent, PoolConfig, PoolError, PoolRegion, pool_schema};

struct Consts;

impl Consts {
    const INITIAL_SAMPLE_BUFFERS: usize = 16;
    const INITIAL_SAMPLE_CAPACITY: usize = 9_216;
    const OVERALL_BYTES: usize = 256 * 1024 * 1024;
}

pool_schema! {
    /// Buffer pools owned by the desktop application composition root.
    pub AppPools {
        bytes: u8,
        samples: f32,
    }
}

/// Concrete buffer-pool facade used by the desktop application.
pub type Pools = PoolRegion<AppPools>;

/// App-owned asset-store shape.
pub type AppStore = kithara::assets::AssetStore<AppPools>;

/// App-owned audio host shape.
pub type AppHost = kithara::host::Host<AppPools>;

/// App-owned playback worker shape.
pub type AppWorker = kithara::play::PlayWorker<AppPools>;

/// App-owned resource configuration shape.
pub type AppResourceConfig<B = kithara::prelude::PlaybackResamplerBackend> =
    kithara::play::ResourceConfig<AppPools, B>;

/// App-owned queue shape.
pub type AppQueue = kithara_queue::Queue<AppPools>;

/// App-owned queue control shape.
pub type AppQueueControl = kithara_queue::QueueControl<AppPools>;

/// App-owned track-source shape.
pub type AppTrackSource = kithara_queue::TrackSource<AppPools>;

/// Build the application's single explicitly registered pool region.
///
/// # Errors
/// Returns an error when pool configuration or initial allocation fails.
pub fn build() -> Result<Pools, PoolError> {
    AppPools::builder(OverallBudget(Consts::OVERALL_BYTES))
        .bytes(
            PoolConfig::builder()
                .initial_buffers(0)
                .max_buffers(usize::MAX)
                .max_share(Percent::FULL)
                .trim_capacity(0)
                .build(),
        )
        .samples(
            PoolConfig::builder()
                .initial_buffers(Consts::INITIAL_SAMPLE_BUFFERS)
                .initial_capacity(Consts::INITIAL_SAMPLE_CAPACITY)
                .max_buffers(128)
                .max_share(Percent::FULL)
                .trim_capacity(200_000)
                .build(),
        )
        .build()
}

#[cfg(test)]
mod tests {
    use std::thread;

    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn initial_samples_are_ready_on_another_thread() {
        let pools = build().unwrap_or_else(|error| panic!("app pool region: {error}"));
        let initial_peak = pools.stats().peak_allocated_bytes;
        let worker_pools = pools.clone();

        let (all_ready, peak) = thread::spawn(move || {
            let buffers = (0..Consts::INITIAL_SAMPLE_BUFFERS)
                .map(|_| {
                    worker_pools
                        .get_with_len::<f32>(Consts::INITIAL_SAMPLE_CAPACITY)
                        .unwrap_or_else(|error| panic!("initial sample buffer: {error}"))
                })
                .collect::<Vec<_>>();
            let all_ready = buffers
                .iter()
                .all(|buffer| buffer.capacity() >= Consts::INITIAL_SAMPLE_CAPACITY);
            let peak = worker_pools.stats().peak_allocated_bytes;
            drop(buffers);
            (all_ready, peak)
        })
        .join()
        .unwrap_or_else(|_| panic!("sample-pool worker panicked"));

        assert!(all_ready);
        assert_eq!(peak, initial_peak);
    }
}
