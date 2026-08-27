use firewheel::FirewheelCtx;
use kithara_platform::sync::mpsc;
pub(crate) use kithara_play::{
    AllocatedSlot, Cmd, PlayerId, PlayerLevel, Reply, SessionDispatcher, SessionError,
    SessionSampleRate,
};
use kithara_play::{PlayError, player::PlayerMember};
use kithara_warp::{
    SyncAdmission, SyncApplied, SyncError, SyncOperation, SyncRejected, SyncStatusSnapshot,
    TopologyOperation,
};

use crate::api::HostLevel;

pub(crate) type StartStreamFn<B> =
    Box<dyn FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send + 'static>;

pub(crate) enum HostCmd {
    Play(Cmd),
    Sync(SyncCmd),
    ApplyMix { levels: Box<[HostLevel]> },
    Shutdown,
}

pub(crate) enum SyncCmd {
    Transact(SyncOperation<PlayerMember>),
    TransactCurrent(Box<[TopologyOperation<PlayerMember>]>),
    Acknowledge(SyncApplied),
}

pub(crate) enum HostReply {
    Play(Reply),
    Admission(Result<SyncAdmission, SyncRejected<PlayerMember>>),
    Acknowledged(Result<SyncStatusSnapshot, SyncError>),
    Ok,
    Err(PlayError),
}

pub(crate) struct HostCmdMsg {
    pub(crate) cmd: HostCmd,
    pub(crate) reply_tx: mpsc::Sender<HostReply>,
}

pub(crate) struct HostDispatchError {
    error: PlayError,
    command: Option<Box<HostCmd>>,
}

impl HostDispatchError {
    pub(crate) fn before_send(error: PlayError, command: HostCmd) -> Self {
        Self {
            error,
            command: Some(Box::new(command)),
        }
    }

    pub(crate) const fn after_send(error: PlayError) -> Self {
        Self {
            error,
            command: None,
        }
    }
}

impl From<HostDispatchError> for PlayError {
    fn from(error: HostDispatchError) -> Self {
        error.error
    }
}

impl From<HostDispatchError> for (PlayError, Option<Box<HostCmd>>) {
    fn from(error: HostDispatchError) -> Self {
        (error.error, error.command)
    }
}

pub(crate) trait HostDispatcher: SessionDispatcher {
    fn exec_host(&self, cmd: HostCmd) -> Result<HostReply, HostDispatchError>;

    fn transact(
        &self,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        match self.exec_host(HostCmd::Sync(SyncCmd::Transact(operation))) {
            Ok(HostReply::Admission(result)) => result,
            Err(error) => {
                let (reason, command) = error.into();
                if let Some(command) = command
                    && let HostCmd::Sync(SyncCmd::Transact(operation)) = *command
                {
                    return Err(SyncRejected::new(SyncError::OwnerUnavailable, operation));
                }
                owner_thread_fail_fast(&reason)
            }
            Ok(_) => owner_thread_fail_fast("unexpected transaction reply"),
        }
    }

    fn transact_current(
        &self,
        operations: Box<[TopologyOperation<PlayerMember>]>,
    ) -> Result<SyncAdmission, PlayError> {
        match self.exec_host(HostCmd::Sync(SyncCmd::TransactCurrent(operations))) {
            Ok(HostReply::Admission(result)) => result.map_err(|rejected| {
                let (error, _) = <(SyncError, SyncOperation<PlayerMember>)>::from(rejected);
                SessionError::from(error).into()
            }),
            Ok(HostReply::Err(error)) => Err(error),
            Err(error) => {
                let (reason, command) = error.into();
                if command.as_deref().is_some_and(|command| {
                    matches!(command, HostCmd::Sync(SyncCmd::TransactCurrent(_)))
                }) {
                    return Err(reason);
                }
                owner_thread_fail_fast(&reason)
            }
            Ok(_) => owner_thread_fail_fast("unexpected current-topology transaction reply"),
        }
    }

    fn acknowledge(&self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        match self.exec_host(HostCmd::Sync(SyncCmd::Acknowledge(applied))) {
            Ok(HostReply::Acknowledged(result)) => result,
            Err(error) => {
                let (reason, command) = error.into();
                if command.as_deref().is_some_and(|command| {
                    matches!(command, HostCmd::Sync(SyncCmd::Acknowledge(_)))
                }) {
                    return Err(SyncError::OwnerUnavailable);
                }
                owner_thread_fail_fast(&reason)
            }
            Ok(_) => owner_thread_fail_fast("unexpected acknowledgement reply"),
        }
    }
}

fn owner_thread_fail_fast(reason: impl std::fmt::Display) -> ! {
    panic!("canonical host owner thread stopped after accepting transferred ownership: {reason}")
}
