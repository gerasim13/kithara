use kithara_platform::thread;
use kithara_test_utils::kithara;

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn receive_is_nonblocking(preloaded: bool, block_on_underrun: bool) -> bool {
    preloaded && !block_on_underrun
}

#[cfg(target_arch = "wasm32")]
pub(super) fn receive_is_nonblocking(_preloaded: bool, _block_on_underrun: bool) -> bool {
    true
}

#[cfg(not(target_arch = "wasm32"))]
#[kithara::allow_block]
pub(super) fn wait_for_fetch(timeout: kithara_platform::time::Duration) {
    thread::park_timeout(timeout);
}

#[cfg(target_arch = "wasm32")]
#[kithara::allow_block]
pub(super) fn wait_for_fetch(timeout: kithara_platform::time::Duration) {
    if thread::is_worker_thread() {
        thread::park_timeout(timeout);
    } else {
        thread::sleep(timeout);
    }
}
