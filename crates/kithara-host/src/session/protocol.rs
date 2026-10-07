use firewheel::FirewheelContext;
use kithara_command::When;
use kithara_output::OutputGroup;
use kithara_platform::sync::mpsc;
use kithara_play::PlayError;
pub(crate) use kithara_play::{
    AllocatedSlot, Cmd, DeckRegistration, PlayerId, Reply, SessionDispatcher, SessionError,
    SessionSampleRate,
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
    Play(Cmd),
    Attach {
        registration: DeckRegistration<S>,
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
    /// The session took the deck and built the slot it plays through.
    Attached(Box<AllocatedSlot>),
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
    /// Adds one deck to the session on its owner thread and starts it there,
    /// answering the slot the deck plays through.
    fn attach(&self, registration: DeckRegistration<S>) -> Result<AllocatedSlot, PlayError> {
        match change_members(self, HostCmd::Attach { registration })? {
            HostReply::Attached(slot) => Ok(*slot),
            _ => owner_thread_fail_fast("unexpected attach reply"),
        }
    }

    /// Stops the deck `grid_id` and removes it from the session on its owner
    /// thread.
    fn detach(&self, grid_id: BeatGridId) -> Result<(), PlayError> {
        match change_members(self, HostCmd::Detach { grid_id })? {
            HostReply::Ok => Ok(()),
            _ => owner_thread_fail_fast("unexpected detach reply"),
        }
    }

    fn exec_host(&self, cmd: HostCmd<S>) -> Result<HostReply, HostDispatchError<S>>;
}

/// Runs one membership change on the owner thread. A change that never
/// reached it fails with the reason; one the owner took and never answered
/// leaves the deck's ownership unknown, so the process stops.
fn change_members<S, D>(dispatcher: &D, cmd: HostCmd<S>) -> Result<HostReply, PlayError>
where
    D: HostDispatcher<S> + ?Sized,
{
    match dispatcher.exec_host(cmd) {
        Ok(HostReply::Err(error)) => Err(error),
        Ok(reply) => Ok(reply),
        Err(error) => {
            let (reason, command) = error.into();
            if command.is_some() {
                return Err(reason);
            }
            owner_thread_fail_fast(&reason)
        }
    }
}

fn owner_thread_fail_fast(reason: impl std::fmt::Display) -> ! {
    panic!("canonical host owner thread stopped after accepting transferred ownership: {reason}")
}
