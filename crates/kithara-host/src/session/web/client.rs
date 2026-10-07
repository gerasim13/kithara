use std::{cell::Cell, marker::PhantomData};

use kithara_bufpool::HasPool;
use kithara_command::{Live, Ticket};
use kithara_platform::{
    maybe_send::MaybeSend,
    sync::{Arc, Mutex},
};

use super::bridge::reset_bridge_state;
use crate::{
    HostCore, HostOwner, HostSettings, PlayError,
    rt::SessionOutput,
    session::{
        dispatch::OwnerPosts,
        protocol::{HostDispatchError, HostDispatcher, HostMailbox, HostPostbox, not_taken},
        queue::HostProtocol,
        state::{HostRoot, RootView},
    },
};

pub(crate) type WebSessionState<O> = Arc<Mutex<Option<O>>>;

enum SessionHost<S, O: HostOwner<S>> {
    Local {
        state: WebSessionState<O>,
        mailbox: Mutex<HostMailbox<O::Command>>,
        posts: Mutex<OwnerPosts>,
    },
    Remote,
}

pub(crate) struct SessionClient<S, O: HostOwner<S>> {
    postbox: HostPostbox<O::Command>,
    host: SessionHost<S, O>,
    marker: PhantomData<fn() -> S>,
}

impl<S, O: HostOwner<S>> HostDispatcher<O::Command> for SessionClient<S, O> {
    fn dispatch(&self, command: O::Command) -> Result<Ticket<PlayError>, HostDispatchError> {
        let ticket = self.postbox.post(command).map_err(not_taken)?;
        if let SessionHost::Local {
            state,
            mailbox,
            posts,
        } = &self.host
        {
            drain_local(state, &mut mailbox.lock(), &mut posts.lock());
        }
        Ok(ticket)
    }
    fn shutdown(&self) {
        if let SessionHost::Local { state, .. } = &self.host {
            state.lock().take();
            WASM_SESSION_ACTIVE.with(|active| active.set(false));
            reset_bridge_state();
        }
    }
}

fn drain_local<S, O: HostOwner<S>>(
    state: &WebSessionState<O>,
    mailbox: &mut HostMailbox<O::Command>,
    posts: &mut OwnerPosts,
) {
    let mut state = state.lock();
    if let Some(owner) = state.as_mut() {
        posts.drain(owner, mailbox);
        posts.pass(owner);
    }
}

thread_local! { static WASM_SESSION_ACTIVE: Cell<bool> = const { Cell::new(false) }; }

pub(crate) fn spawn<S, O>(
    _root: HostRoot,
    _view: RootView,
    _output: SessionOutput,
    _settings: Live<HostSettings, HostProtocol>,
    _layer: impl FnOnce(HostCore<S, O::Deck>) -> O + MaybeSend + 'static,
) -> Result<(Arc<dyn HostDispatcher<O::Command>>, WebSessionState<O>), PlayError>
where
    S: HasPool<f32> + Send + Sync + 'static,
    O: HostOwner<S>,
{
    todo!(
        "Construct the browser SessionState and its single dispatcher channel, then store layer(core) on its owning thread with canonical deck postbox wakes (spec §4.1, §5.3)"
    )
}

pub(crate) fn remote<S, O: HostOwner<S>>(
    postbox: HostPostbox<O::Command>,
) -> Arc<dyn HostDispatcher<O::Command>> {
    Arc::new(SessionClient {
        postbox,
        host: SessionHost::Remote,
        marker: PhantomData,
    })
}
