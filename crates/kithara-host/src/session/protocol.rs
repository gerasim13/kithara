use firewheel::FirewheelContext;
use kithara_command::When;
use kithara_output::OutputGroup;
use kithara_platform::sync::mpsc;
use kithara_play::PlayError;
pub(crate) use kithara_play::{
    AllocatedSlot, Cmd, PlayerId, Reply, SessionDispatcher, SessionError, SessionSampleRate,
};
use kithara_signal::SessionFrame;
use kithara_warp::BeatGridId;

use crate::{api::Tap, host::HostSettingsChange};

/// Opens the audio stream a session runs on and hands back the object that
/// owns it. Firewheel no longer holds the backend, so the session keeps the
/// returned stream alive for as long as its context.
pub(crate) type StartStreamFn<T> =
    Box<dyn FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static>;

pub(crate) enum HostCmd<S> {
    Play(Cmd<S>),
    Attach {
        grid_id: BeatGridId,
    },
    Detach {
        grid_id: BeatGridId,
    },
    Configure {
        change: HostSettingsChange,
        at: When<SessionFrame>,
    },
    AttachOutputs {
        tap: Tap,
        outputs: OutputGroup,
    },
    DetachOutputs {
        tap: Tap,
    },
    /// The platform moved the output to another route: the stream restarts
    /// and every deck hears the change.
    InvalidateAudioRoute {
        reason: String,
    },
    Shutdown,
}

pub(crate) enum HostReply {
    Play(Reply),
    Ok,
    Err(PlayError),
}

pub(crate) struct HostCmdMsg<S> {
    pub(crate) cmd: HostCmd<S>,
    pub(crate) reply_tx: mpsc::Sender<HostReply>,
}

pub(crate) struct HostDispatchError<S> {
    command: Option<Box<HostCmd<S>>>,
    error: PlayError,
}

impl<S> HostDispatchError<S> {
    pub(crate) const fn after_send(error: PlayError) -> Self {
        Self {
            error,
            command: None,
        }
    }

    pub(crate) fn before_send(error: PlayError, command: HostCmd<S>) -> Self {
        Self {
            error,
            command: Some(Box::new(command)),
        }
    }
}

impl<S> From<HostDispatchError<S>> for PlayError {
    fn from(error: HostDispatchError<S>) -> Self {
        error.error
    }
}

impl<S> From<HostDispatchError<S>> for (PlayError, Option<Box<HostCmd<S>>>) {
    fn from(error: HostDispatchError<S>) -> Self {
        (error.error, error.command)
    }
}

pub(crate) trait HostDispatcher<S>: SessionDispatcher<S> {
    /// Adds one deck to the session on its owner thread.
    fn attach(&self, grid_id: BeatGridId) -> Result<(), PlayError> {
        change_members(self, HostCmd::Attach { grid_id })
    }

    /// Removes the deck `grid_id` from the session on its owner thread.
    fn detach(&self, grid_id: BeatGridId) -> Result<(), PlayError> {
        change_members(self, HostCmd::Detach { grid_id })
    }

    fn exec_host(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>>;
}

/// Runs one membership change on the owner thread. A change that never
/// reached it fails with the reason; one the owner took and never answered
/// leaves the deck's ownership unknown, so the process stops.
fn change_members<S, D>(dispatcher: &D, cmd: HostCmd<S>) -> Result<(), PlayError>
where
    D: HostDispatcher<S> + ?Sized,
{
    match dispatcher.exec_host(cmd) {
        Ok(HostReply::Ok) => Ok(()),
        Ok(HostReply::Err(error)) => Err(error),
        Err(error) => {
            let (reason, command) = error.into();
            if command.is_some() {
                return Err(reason);
            }
            owner_thread_fail_fast(&reason)
        }
        Ok(HostReply::Play(_)) => owner_thread_fail_fast("unexpected membership reply"),
    }
}

fn owner_thread_fail_fast(reason: impl std::fmt::Display) -> ! {
    panic!("canonical host owner thread stopped after accepting transferred ownership: {reason}")
}
