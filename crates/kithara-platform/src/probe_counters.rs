use std::sync::atomic::{AtomicU64, Ordering};

pub fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::SeqCst);
}

#[must_use]
pub fn read(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::SeqCst)
}

pub static DL_SPAWN_CALLED: AtomicU64 = AtomicU64::new(0);
pub static DL_WORKER_ENTERED: AtomicU64 = AtomicU64::new(0);
pub static DL_RUN_ENTERED: AtomicU64 = AtomicU64::new(0);
pub static PEER_CMD_SENT: AtomicU64 = AtomicU64::new(0);
pub static PEER_RESP_RECEIVED: AtomicU64 = AtomicU64::new(0);
pub static NET_SEND_STARTED: AtomicU64 = AtomicU64::new(0);
pub static NET_SEND_DONE: AtomicU64 = AtomicU64::new(0);
