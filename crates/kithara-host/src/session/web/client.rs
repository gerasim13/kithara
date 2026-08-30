use std::{cell::RefCell, num::NonZeroU32};

use kithara_audio::ConsumerWakeMode;
use kithara_platform::sync::{Arc, mpsc};
use kithara_play::{GroupState, player::PlayerMember};

use super::bridge::{init_bridge_state, reset_bridge_state, start_stream_web_audio};
use crate::{
    error::PlayError,
    session::{
        dispatch::run_host_cmd,
        protocol::{
            Cmd, HostCmd, HostCmdMsg, HostDispatchError, HostDispatcher, HostReply, Reply,
            SessionDispatcher,
        },
        state::{RootView, SessionState},
    },
};

enum SessionHost {
    Local,
    Remote { tx: mpsc::Sender<HostCmdMsg> },
}

pub(crate) struct SessionClient {
    host: SessionHost,
}

impl SessionClient {
    fn call(&self, cmd: HostCmd) -> Result<HostReply, HostDispatchError> {
        match &self.host {
            SessionHost::Local => WASM_SESSION_STATE.with(|cell| {
                if matches!(&cmd, HostCmd::Shutdown) {
                    drop(cell.borrow_mut().take());
                    reset_bridge_state();
                    return Ok(HostReply::Ok);
                }
                let mut state = cell.borrow_mut();
                match state.as_mut() {
                    Some(state) => Ok(run_host_cmd(state, cmd)),
                    None => Err(HostDispatchError::before_send(
                        PlayError::Internal("local session state missing".into()),
                        cmd,
                    )),
                }
            }),
            SessionHost::Remote { tx } => {
                let (reply_tx, reply_rx) = mpsc::channel();
                if let Err(error) = tx.send(HostCmdMsg { cmd, reply_tx }) {
                    return Err(HostDispatchError::before_send(
                        PlayError::SessionGone {
                            reason: "session host stopped accepting commands",
                        },
                        error.0.cmd,
                    ));
                }
                reply_rx.recv().map_err(|_| {
                    HostDispatchError::after_send(PlayError::SessionGone {
                        reason: "session host dropped the reply channel",
                    })
                })
            }
        }
    }
}

impl SessionDispatcher for SessionClient {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError> {
        match self.call(HostCmd::Play(cmd)).map_err(PlayError::from)? {
            HostReply::Play(reply) => Ok(reply),
            HostReply::Err(error) => Err(error),
            _ => Err(PlayError::Internal(
                "unexpected host reply for player session command".into(),
            )),
        }
    }
}

impl HostDispatcher for SessionClient {
    fn exec_host(&self, cmd: HostCmd) -> Result<HostReply, HostDispatchError> {
        self.call(cmd)
    }
}

thread_local! {
    pub(super) static WASM_SESSION_STATE: RefCell<Option<SessionState<firewheel_web_audio::WebAudioBackend>>> = const { RefCell::new(None) };
}

pub(crate) fn spawn(
    root: GroupState<PlayerMember>,
    root_view: RootView,
    sample_rate: NonZeroU32,
) -> Result<Arc<dyn HostDispatcher>, PlayError> {
    WASM_SESSION_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.is_some() {
            return Err(PlayError::SessionAlreadyActive);
        }
        *state = Some(SessionState::new(
            root,
            root_view,
            sample_rate,
            start_stream_web_audio,
        ));
        Ok(())
    })?;
    init_bridge_state();
    let client = Arc::new(SessionClient {
        host: SessionHost::Local,
    });
    Ok(client)
}

pub(crate) fn remote(tx: mpsc::Sender<HostCmdMsg>) -> Arc<dyn HostDispatcher> {
    let client = Arc::new(SessionClient {
        host: SessionHost::Remote { tx },
    });
    client
}

pub(crate) fn worker_channel() -> (mpsc::Sender<HostCmdMsg>, mpsc::Receiver<HostCmdMsg>) {
    mpsc::channel()
}
