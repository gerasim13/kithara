use firewheel::FirewheelContext;
use kithara_audio::ConsumerWakeMode;
use kithara_command::{Mailbox, PostError, Postbox, Refused, Ticket, When};
use kithara_output::OutputGroup;
use kithara_platform::maybe_send::{MaybeSend, MaybeSync};
use kithara_play::PlayError;
pub(crate) use kithara_play::{
    AllocatedSlot, DeckRegistration, PlayerId, SessionError, SessionSampleRate,
};
use kithara_signal::SessionFrame;
use kithara_warp::BeatGridId;

use crate::{
    api::{SlotId, Tap},
    bridge::{NodeInputs, slot_channels},
    host::HostSettingsChange,
};

/// Opens the audio stream a session runs on and hands back the object that
/// owns it. Firewheel no longer holds the backend, so the session keeps the
/// returned stream alive for as long as its context.
pub(crate) type StartStreamFn<T> =
    Box<dyn FnMut(&mut FirewheelContext, u32) -> Result<T, String> + Send + 'static>;

/// Where the Host posts its session's commands; each post is answered applied
/// or refused with the session's error.
pub(crate) type HostPostbox<S> = Postbox<HostCmd<S>, PlayError>;

/// What a session's owner drains the Host's commands from.
pub(crate) type HostMailbox<S> = Mailbox<HostCmd<S>, PlayError>;

/// What the Host asks of its session. The session's owner runs the commands
/// one at a time, in the order they were posted, and answers each one.
pub(crate) enum HostCmd<S> {
    /// Takes a deck and starts it on the render half of its slot.
    Attach {
        registration: Box<DeckRegistration<S>>,
        inputs: NodeInputs,
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

impl<S> HostCmd<S> {
    /// The command that starts `registration`'s deck, with the slot the deck
    /// is seated on once the session applies it.
    pub(crate) fn attach(registration: DeckRegistration<S>) -> (Self, AllocatedSlot) {
        let (inputs, control) = slot_channels();
        (
            Self::Attach {
                registration: Box::new(registration),
                inputs,
            },
            AllocatedSlot::new(control, SlotId::new(1)),
        )
    }
}

/// Why a Host command did not apply.
pub(crate) enum HostDispatchError {
    /// The session stopped taking commands before this one reached it.
    NotTaken(PlayError),
    /// The session refused it.
    Refused(PlayError),
    /// The session took the command and dropped it unanswered.
    Unanswered,
}

impl From<HostDispatchError> for PlayError {
    fn from(error: HostDispatchError) -> Self {
        match error {
            HostDispatchError::NotTaken(error) | HostDispatchError::Refused(error) => error,
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
        let (command, slot) = HostCmd::attach(registration);
        change_members(self, command)?;
        Ok(slot)
    }

    /// Stops the deck `grid_id` and removes it from the session on its owner
    /// thread.
    fn detach(&self, grid_id: BeatGridId) -> Result<(), PlayError> {
        change_members(self, HostCmd::Detach { grid_id })
    }

    /// How the audio consumers of the decks this session hosts may wake
    /// workers. Every one of them reads from the render callback, offline
    /// backends included.
    fn consumer_wake_mode(&self) -> ConsumerWakeMode;

    /// Posts `cmd` to the session's owner, answering the ticket its answer
    /// comes to.
    ///
    /// # Errors
    /// [`HostDispatchError::NotTaken`] when the session stopped taking
    /// commands.
    fn dispatch(&self, cmd: HostCmd<S>) -> Result<Ticket<PlayError>, HostDispatchError>;
}

/// Dispatches `cmd` and waits for the session's answer.
pub(crate) fn ask<S, D>(dispatcher: &D, cmd: HostCmd<S>) -> Result<(), HostDispatchError>
where
    D: HostDispatcher<S> + ?Sized,
{
    dispatcher
        .dispatch(cmd)?
        .wait()
        .map_err(|refused| match refused {
            Refused::Owner(error) => HostDispatchError::Refused(error),
            Refused::Unanswered => HostDispatchError::Unanswered,
        })
}

/// The session stopped taking commands: posting found its mailbox gone.
pub(crate) fn not_taken(_: PostError) -> HostDispatchError {
    HostDispatchError::NotTaken(PlayError::SessionGone {
        reason: "the session stopped taking commands",
    })
}

/// Runs one membership change on the owner thread. A change that never
/// reached it, or that it refused, fails with the reason; one the owner took
/// and never answered leaves the deck's ownership unknown, so the process
/// stops.
fn change_members<S, D>(dispatcher: &D, command: HostCmd<S>) -> Result<(), PlayError>
where
    D: HostDispatcher<S> + ?Sized,
{
    match ask(dispatcher, command) {
        Ok(()) => Ok(()),
        Err(HostDispatchError::NotTaken(reason) | HostDispatchError::Refused(reason)) => {
            Err(reason)
        }
        Err(error @ HostDispatchError::Unanswered) => {
            owner_thread_fail_fast(PlayError::from(error))
        }
    }
}

fn owner_thread_fail_fast(reason: impl std::fmt::Display) -> ! {
    panic!("canonical host owner thread stopped after accepting transferred ownership: {reason}")
}

#[cfg(test)]
mod tests {
    use kithara_command::{Post, mailbox};
    use kithara_platform::sync::Mutex;
    use kithara_test_utils::{bufpool::TestPools, kithara};

    use super::*;

    /// A session that drains each post as it lands and answers it with what
    /// `outcome` gives, or drops it unanswered for `None`.
    struct Session {
        postbox: HostPostbox<TestPools>,
        mailbox: Mutex<HostMailbox<TestPools>>,
        outcome: fn() -> Option<Result<(), PlayError>>,
    }

    impl HostDispatcher<TestPools> for Session {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }

        fn dispatch(
            &self,
            cmd: HostCmd<TestPools>,
        ) -> Result<Ticket<PlayError>, HostDispatchError> {
            let ticket = self.postbox.post(cmd).map_err(not_taken)?;
            for Post { answer, .. } in self.mailbox.lock().drain() {
                if let Some(outcome) = (self.outcome)() {
                    answer.answer(outcome);
                }
            }
            Ok(ticket)
        }
    }

    fn session(outcome: fn() -> Option<Result<(), PlayError>>) -> Session {
        let (postbox, mailbox) = mailbox();
        Session {
            postbox,
            mailbox: Mutex::new(mailbox),
            outcome,
        }
    }

    #[kithara::test]
    fn a_post_the_session_drops_reads_unanswered() {
        let asked = ask(&session(|| None), HostCmd::Shutdown);

        assert!(matches!(asked, Err(HostDispatchError::Unanswered)));
    }

    #[kithara::test]
    fn the_session_refusal_reaches_the_caller() {
        let asked = ask(&session(|| Some(Err(PlayError::Late))), HostCmd::Shutdown);

        assert!(matches!(
            asked,
            Err(HostDispatchError::Refused(PlayError::Late))
        ));
    }
}
