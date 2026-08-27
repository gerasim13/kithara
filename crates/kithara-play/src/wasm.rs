use kithara_bufpool::PcmPool;
use kithara_platform::thread::assert_main_thread;

/// Start the main-thread WebCodecs capability probe.
pub fn spawn_webcodecs_probe(pcm_pool: PcmPool) {
    assert_main_thread("spawn_webcodecs_probe");
    kithara_decode::spawn_webcodecs_probe(pcm_pool);
}
