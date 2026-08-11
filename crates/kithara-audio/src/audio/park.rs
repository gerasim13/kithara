#[cfg(not(target_arch = "wasm32"))]
pub(super) const fn receive_is_nonblocking(preloaded: bool, block_on_underrun: bool) -> bool {
    preloaded && !block_on_underrun
}

#[cfg(target_arch = "wasm32")]
pub(super) fn receive_is_nonblocking(_preloaded: bool, _block_on_underrun: bool) -> bool {
    true
}
