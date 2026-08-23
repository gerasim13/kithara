#[cfg(not(target_arch = "wasm32"))]
use kithara_platform::tokio::runtime::Handle;
use kithara_platform::{
    sync::{Arc, Weak},
    time::{self, Instant},
    tokio::{select, task},
};

use super::{
    core::{AbrController, AbrPeerId},
    peer::PeerEntry,
};

impl AbrController {
    pub(super) fn defer_tick(
        self: &Arc<Self>,
        peer_id: AbrPeerId,
        entry: &Arc<PeerEntry>,
        deadline: Instant,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let Ok(runtime) = Handle::try_current() else {
            return;
        };

        if !entry.arm_deferred_tick(deadline) {
            return;
        }

        let cancel = entry.cancel.clone();
        let controller = Arc::downgrade(self);
        let entry = Arc::downgrade(entry);
        let deferred_tick = async move {
            let delay = deadline.saturating_duration_since(Instant::now());
            select! {
                biased;
                () = cancel.cancelled() => {}
                () = time::sleep(delay) => Self::run_deferred_tick(
                    &controller,
                    &entry,
                    peer_id,
                    deadline,
                ),
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        drop(task::spawn_on(&runtime, deferred_tick));
        #[cfg(target_arch = "wasm32")]
        drop(task::spawn(deferred_tick));
    }

    fn run_deferred_tick(
        controller: &Weak<Self>,
        entry: &Weak<PeerEntry>,
        peer_id: AbrPeerId,
        deadline: Instant,
    ) {
        let Some(entry) = entry.upgrade() else {
            return;
        };
        if !entry.take_deferred_tick(deadline) {
            return;
        }
        let Some(controller) = controller.upgrade() else {
            return;
        };
        controller.tick(peer_id, Instant::now());
    }
}
