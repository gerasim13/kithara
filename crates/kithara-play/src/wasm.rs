use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::thread::assert_main_thread;

/// Start the main-thread WebCodecs capability probe.
pub fn spawn_webcodecs_probe<S>(pools: PoolRegion<S>)
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    assert_main_thread("spawn_webcodecs_probe");
    kithara_decode::spawn_webcodecs_probe(pools);
}
