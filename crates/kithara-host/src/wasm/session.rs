use crate::{
    Host, HostCommand, HostCore, HostOwner, PlayError,
    session::{
        RootView,
        dispatch::OwnerPosts,
        protocol::{HostMailbox, HostPostbox},
        web::WebSessionState,
    },
};
use kithara_bufpool::HasPool;
use kithara_platform::{
    sync::{Arc, Mutex},
    thread::{assert_main_thread, assert_not_main_thread},
};

/// A Worker endpoint to the canonical owner's existing command queue.
#[derive_where::derive_where(Clone)]
pub struct HostSender<S> {
    id: crate::DeckId,
    root_view: RootView,
    postbox: HostPostbox<HostCommand<S, dyn kithara_play::HostedDeck<S>>>,
}

/// The main-thread endpoint driving the same canonical owner.
pub struct HostReceiver<S> {
    state: WebSessionState<HostCore<S>>,
    route: Arc<HostRoute<S>>,
}

pub(crate) struct HostRoute<S> {
    mailbox: Mutex<Option<HostMailbox<HostCommand<S, dyn kithara_play::HostedDeck<S>>>>>,
    posts: Mutex<OwnerPosts>,
}

impl<S> HostRoute<S> {
    pub(crate) fn close(&self) {
        self.mailbox.lock().take();
    }
}

/// Shares the existing owner's route with a Worker without transferring decks twice.
pub fn worker_host_channel<S: HasPool<f32> + Send + Sync + 'static>(
    _host: &Host<S>,
) -> Result<(HostSender<S>, HostReceiver<S>), PlayError> {
    assert_main_thread("worker_host_channel");
    todo!(
        "Clone the canonical owner route, with a transfer-safe deck registration command; no second Worker Decks owner (spec §4.1, §5.3)"
    )
}

/// Connects a Worker facade to the existing main-thread owner.
pub fn remote_host<S: HasPool<f32> + Send + Sync + 'static>(_sender: HostSender<S>) -> Host<S> {
    assert_not_main_thread("remote_host");
    todo!(
        "Build a non-owning Host handle over the existing owner postbox and published snapshot (spec §4.1)"
    )
}

/// Warms the local browser backend through its owner.
pub fn warm_up_audio<S: HasPool<f32> + Send + Sync + 'static>(
    host: &Host<S>,
) -> Result<(), PlayError> {
    assert_main_thread("warm_up_audio");
    let state = host
        .session
        .platform()
        .web_state
        .as_ref()
        .ok_or_else(|| PlayError::Internal("audio warm-up requires a local host".to_owned()))?;
    crate::session::warm_up_audio(state).map_err(Into::into)
}

/// Runs pending owner commands and one owner pass on the main thread.
pub fn tick_and_poll<S: HasPool<f32> + Send + Sync + 'static>(receiver: &HostReceiver<S>) {
    assert_main_thread("tick_and_poll");
    if let Some(mailbox) = receiver.route.mailbox.lock().as_mut() {
        crate::session::tick_and_poll_remote(
            &receiver.state,
            mailbox,
            &mut receiver.route.posts.lock(),
        );
    }
}
