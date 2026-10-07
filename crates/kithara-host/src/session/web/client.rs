use std::cell::Cell;

use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::sync::{Arc, Mutex, mpsc};

use super::bridge::{init_bridge_state, reset_bridge_state, start_stream_web_audio};
use crate::{
    HostSettings,
    error::PlayError,
    rt::SessionOutput,
    session::{
        HostProtocol,
        dispatch::run_host_cmd,
        protocol::{HostCmd, HostDispatchError, HostDispatcher, answer},
        state::{HostRoot, RootView, SessionState},
    },
};

pub(crate) type WebSessionState<S> =
    Arc<Mutex<Option<SessionState<firewheel_web_audio::WebAudioBackend, S>>>>;

enum SessionHost<S> {
    Local { state: WebSessionState<S> },
    Remote { tx: mpsc::Sender<HostCmd<S>> },
}

pub(crate) struct SessionClient<S> {
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
    /// time this returns; a remote one sends it to the session's thread.
    fn dispatch(&self, cmd: HostCmd<S>) -> Result<(), HostDispatchError> {
        match &self.host {
            SessionHost::Local { state } => {
                if let HostCmd::Shutdown(reply) = cmd {
                    drop(state.lock().take());
                    WASM_SESSION_ACTIVE.with(|active| active.set(false));
                    reset_bridge_state();
                    answer(&reply, ());
                    return Ok(());
                }
                let mut state = state.lock();
                let state = state.as_mut().ok_or_else(|| {
                    HostDispatchError::NotTaken(PlayError::Internal(
                        "local session state missing".into(),
                    ))
                })?;
                run_host_cmd(state, cmd);
                Ok(())
            }
            SessionHost::Remote { tx } => tx.send(cmd).map_err(|_| {
                HostDispatchError::NotTaken(PlayError::SessionGone {
                    reason: "session host stopped accepting commands",
                })
            }),
        }
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
    let client = Arc::new(SessionClient {
        host: SessionHost::Local {
            state: Arc::clone(&state),
        },
    });
    Ok((client, state))
}

pub(crate) fn remote<S: HasPool<f32> + Send + Sync + 'static>(
    tx: mpsc::Sender<HostCmd<S>>,
) -> Arc<dyn HostDispatcher<S>> {
    Arc::new(SessionClient {
        host: SessionHost::Remote { tx },
    })
}

pub(crate) fn worker_channel<S>() -> (mpsc::Sender<HostCmd<S>>, mpsc::Receiver<HostCmd<S>>) {
    mpsc::channel()
}
