use std::cell::Cell;

use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::HasPool;
use kithara_command::{Live, Post, Ticket};
use kithara_platform::sync::{Arc, Mutex};

use super::bridge::{init_bridge_state, reset_bridge_state, start_stream_web_audio};
use crate::{
    HostSettings,
    error::PlayError,
    rt::SessionOutput,
    session::{
        HostProtocol,
        dispatch::run_host_cmd,
        protocol::{
            HostCmd, HostDispatchError, HostDispatcher, HostMailbox, HostPostbox, not_taken,
        },
        state::{HostRoot, RootView, SessionState},
    },
};

pub(crate) type WebSessionState<S> =
    Arc<Mutex<Option<SessionState<firewheel_web_audio::WebAudioBackend, S>>>>;

enum SessionHost<S> {
    /// The session lives on this thread and drains each post as it lands.
    Local {
        state: WebSessionState<S>,
        mailbox: Mutex<HostMailbox<S>>,
    },
    /// The session lives on the main thread, which drains on its frame tick.
    Remote,
}

pub(crate) struct SessionClient<S> {
    postbox: HostPostbox<S>,
    host: SessionHost<S>,
}

impl<S> HostDispatcher<S> for SessionClient<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    /// A local session runs the command inline, so its answer is there by the
    /// time this returns; a remote one answers on the main thread's next tick.
    fn dispatch(&self, cmd: HostCmd<S>) -> Result<Ticket<PlayError>, HostDispatchError> {
        let ticket = self.postbox.post(cmd).map_err(not_taken)?;
        if let SessionHost::Local { state, mailbox } = &self.host {
            drain_local(state, &mut mailbox.lock());
        }
        Ok(ticket)
    }
}

/// Runs and answers every post the local session holds. Shutdown drops the
/// session; a post after it is refused.
fn drain_local<S>(state: &WebSessionState<S>, mailbox: &mut HostMailbox<S>)
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    for Post { command, answer } in mailbox.drain() {
        if matches!(command, HostCmd::Shutdown) {
            drop(state.lock().take());
            WASM_SESSION_ACTIVE.with(|active| active.set(false));
            reset_bridge_state();
            answer.answer(Ok(()));
            continue;
        }
        let mut state = state.lock();
        let outcome = state.as_mut().map_or_else(
            || Err(PlayError::Internal("local session state missing".into())),
            |state| run_host_cmd(state, command),
        );
        answer.answer(outcome);
    }
}

thread_local! {
    static WASM_SESSION_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn spawn<S: HasPool<f32> + Send + Sync + 'static>(
    root: HostRoot,
    root_view: RootView,
    output: SessionOutput,
    settings: Live<HostSettings, HostProtocol>,
) -> Result<(Arc<dyn HostDispatcher<S>>, WebSessionState<S>), PlayError> {
    WASM_SESSION_ACTIVE.with(|active| {
        if active.replace(true) {
            return Err(PlayError::SessionAlreadyActive);
        }
        Ok(())
    })?;
    let mut session = SessionState::new(
        root,
        root_view.clone(),
        None,
        None,
        output,
        settings,
        start_stream_web_audio,
    );
    // A browser unlocks its output through a user gesture and can never resume
    // a closed `AudioContext`: releasing the device on idle is irreversible, so
    // every later context stays suspended and the render callback never runs
    // again. This session holds its device for as long as it lives.
    session.retains_output = true;
    let state = Arc::new(Mutex::new(Some(session)));
    init_bridge_state();
    let (postbox, mailbox) = kithara_command::mailbox();
    let client = Arc::new(SessionClient {
        postbox,
        host: SessionHost::Local {
            state: Arc::clone(&state),
            mailbox: Mutex::new(mailbox),
        },
    });
    Ok((client, state))
}

pub(crate) fn remote<S: HasPool<f32> + Send + Sync + 'static>(
    postbox: HostPostbox<S>,
) -> Arc<dyn HostDispatcher<S>> {
    Arc::new(SessionClient {
        postbox,
        host: SessionHost::Remote,
    })
}
