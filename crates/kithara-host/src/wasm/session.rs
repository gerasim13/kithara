use kithara_platform::{
    sync::{Arc, Mutex, mpsc},
    thread::assert_main_thread,
};
use kithara_warp::BeatGridId;

use crate::{
    Host,
    session::{self as host_session, RootView, protocol::HostCmdMsg},
};

/// Worker-side endpoint for the canonical Host owned by the main thread.
#[derive(Clone)]
pub struct HostSender {
    id: BeatGridId,
    root_view: RootView,
    tx: mpsc::Sender<HostCmdMsg>,
}

/// Main-thread receiver for one canonical Host command route.
pub struct HostReceiver {
    route: Arc<HostRoute>,
}

pub(crate) struct HostRoute {
    receiver: Mutex<Option<mpsc::Receiver<HostCmdMsg>>>,
}

impl HostRoute {
    fn new(receiver: mpsc::Receiver<HostCmdMsg>) -> Self {
        Self {
            receiver: Mutex::new(Some(receiver)),
        }
    }

    pub(crate) fn close(&self) {
        self.receiver.lock().take();
    }
}

/// Creates the Worker route for an already constructed main-thread Host.
#[must_use]
pub fn worker_host_channel(host: &Host) -> (HostSender, HostReceiver) {
    assert_main_thread("worker_host_channel");
    let (id, root_view) = host.remote_identity();
    let (tx, rx) = host_session::worker_channel();
    let route = Arc::new(HostRoute::new(rx));
    host.register_remote_route(Arc::clone(&route));
    (HostSender { id, root_view, tx }, HostReceiver { route })
}

/// Connects a Worker facade to the main thread's canonical Host owner.
#[must_use]
pub fn remote_host(sender: HostSender) -> Host {
    let dispatcher = host_session::remote(sender.tx);
    Host::remote(sender.id, sender.root_view, dispatcher)
}

/// Pre-initialise the audio context and AudioWorklet eagerly.
///
/// Call on the main thread after constructing [`Host`]. This creates the
/// AudioContext in suspended state and starts the async AudioWorklet module
/// load. Once complete, `firewheel-web-audio` registers auto-resume listeners
/// so that the very first user click resumes the context.
pub fn warm_up_audio() {
    assert_main_thread("warm_up_audio");
    host_session::warm_up_audio();
}

/// Poll pending session commands from Workers and update the audio graph.
///
/// Call this on the main thread from `requestAnimationFrame`.
pub fn tick_and_poll(receiver: &HostReceiver) {
    assert_main_thread("tick_and_poll");
    let route = receiver.route.receiver.lock();
    if let Some(rx) = route.as_ref() {
        host_session::tick_and_poll_remote(rx);
    }
}
