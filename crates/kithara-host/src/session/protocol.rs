use firewheel::FirewheelContext;
use kithara_audio::ConsumerWakeMode;
use kithara_command::When;
use kithara_output::OutputGroup;
use kithara_platform::{
    maybe_send::{MaybeSend, MaybeSync},
    sync::mpsc,
};
use kithara_play::PlayError;
pub(crate) use kithara_play::{
    AllocatedSlot, DeckRegistration, PlayerId, SessionError, SessionSampleRate,
};
use kithara_signal::SessionFrame;
use kithara_warp::BeatGridId;
use tracing::warn;

use crate::{api::Tap, host::HostSettingsChange};

/// Opens the audio stream a session runs on and hands back the object that
/// owns it. Firewheel no longer holds the backend, so the session keeps the
/// returned stream alive for as long as its context.
pub(crate) type StartStreamFn<T> =
    Box<dyn FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static>;

/// Where the session sends a command's answer; the caller waits on the other
/// end.
pub(crate) type Reply<T> = mpsc::Sender<T>;

/// What the Host asks of its session. Each command carries the reply its
/// answer goes to; the session's owner runs commands one at a time, in the
/// order they were dispatched.
pub(crate) enum HostCmd<S> {
    /// Takes a deck and starts it, answering the slot it plays through.
    Attach {
        registration: DeckRegistration<S>,
        reply: Reply<Result<AllocatedSlot, PlayError>>,
    },
    Detach {
        grid_id: BeatGridId,
        reply: Reply<Result<(), PlayError>>,
    },
    Configure {
        change: HostSettingsChange,
        at: When<SessionFrame>,
        reply: Reply<Result<(), PlayError>>,
    },
    AttachOutputs {
        tap: Tap,
        outputs: OutputGroup,
        reply: Reply<Result<(), PlayError>>,
    },
    DetachOutputs {
        tap: Tap,
        reply: Reply<()>,
    },
    /// The platform moved the output to another route: the stream restarts
    /// and every deck hears the change.
    InvalidateAudioRoute {
        reason: String,
        reply: Reply<Result<(), PlayError>>,
    },
    Shutdown(Reply<()>),
}

/// Why a Host command came back without its answer.
pub(crate) enum HostDispatchError {
    /// The session stopped taking commands before this one reached it.
    NotTaken(PlayError),
    /// The session took the command and dropped it unanswered.
    Unanswered,
}

impl From<HostDispatchError> for PlayError {
    fn from(error: HostDispatchError) -> Self {
        match error {
            HostDispatchError::NotTaken(error) => error,
            HostDispatchError::Unanswered => Self::SessionGone {
                reason: "the session dropped a command unanswered",
            },
        }
    }
}

pub(crate) trait HostDispatcher<S>: MaybeSend + MaybeSync {
    /// Adds one deck to the session on its owner thread and starts it there,
    /// answering the slot the deck plays through.
    fn attach(&self, registration: DeckRegistration<S>) -> Result<AllocatedSlot, PlayError> {
        change_members(self, |reply| HostCmd::Attach {
            registration,
            reply,
        })
    }

    /// Stops the deck `grid_id` and removes it from the session on its owner
    /// thread.
    fn detach(&self, grid_id: BeatGridId) -> Result<(), PlayError> {
        change_members(self, |reply| HostCmd::Detach { grid_id, reply })
    }

    /// How the audio consumers of the decks this session hosts may wake
    /// workers. Every one of them reads from the render callback, offline
    /// backends included.
    fn consumer_wake_mode(&self) -> ConsumerWakeMode;

    /// Hands `cmd` to the session's owner, which answers it through the reply
    /// the command carries.
    ///
    /// # Errors
    /// [`HostDispatchError::NotTaken`] when the session stopped taking
    /// commands.
    fn dispatch(&self, cmd: HostCmd<S>) -> Result<(), HostDispatchError>;
}

/// Dispatches the command `command` builds around its reply and waits for the
/// answer.
pub(crate) fn ask<S, T, D>(
    dispatcher: &D,
    command: impl FnOnce(Reply<T>) -> HostCmd<S>,
) -> Result<T, HostDispatchError>
where
    D: HostDispatcher<S> + ?Sized,
{
    let (reply, answer) = mpsc::channel();
    dispatcher.dispatch(command(reply))?;
    answer.recv().map_err(|_| HostDispatchError::Unanswered)
}

/// Sends `value` to the caller waiting on `reply`; a caller that stopped
/// waiting only loses the answer.
pub(crate) fn answer<T>(reply: &Reply<T>, value: T) {
    if reply.send(value).is_err() {
        warn!("[KITHARA-ROUTE] a Host command's caller stopped waiting for its answer");
    }
}

/// Runs one membership change on the owner thread. A change that never
/// reached it fails with the reason; one the owner took and never answered
/// leaves the deck's ownership unknown, so the process stops.
fn change_members<S, T, D>(
    dispatcher: &D,
    command: impl FnOnce(Reply<Result<T, PlayError>>) -> HostCmd<S>,
) -> Result<T, PlayError>
where
    D: HostDispatcher<S> + ?Sized,
{
    match ask(dispatcher, command) {
        Ok(answer) => answer,
        Err(HostDispatchError::NotTaken(reason)) => Err(reason),
        Err(error @ HostDispatchError::Unanswered) => {
            owner_thread_fail_fast(PlayError::from(error))
        }
    }
}

fn owner_thread_fail_fast(reason: impl std::fmt::Display) -> ! {
    panic!("canonical host owner thread stopped after accepting transferred ownership: {reason}")
}
