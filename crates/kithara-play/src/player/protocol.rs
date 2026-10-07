use std::task::Waker;

use kithara_bufpool::HasPool;
use kithara_platform::maybe_send::{MaybeSend, MaybeSync};

use super::{PlayerImpl, PlayerRuntime};
use crate::{AllocatedSlot, DeckRegistration, PlayError, SessionBinding};

/// What an executor that holds a player or decorator calls on it.
///
/// The executor owns the player for as long as it holds it: it runs the
/// commands the player's handles post when the player wakes it, and ticks it
/// at the executor's pace. Item, EQ, volume, and event APIs stay on the
/// concrete handle; the session binds through
/// [`PlayerControlSource::attach_session`]. Only its holder reaches it, so a
/// player need not be shared between threads.
pub trait Player: MaybeSend + 'static {
    /// Stop owned work and detach the player from its playback session.
    fn close(&mut self) -> Result<(), PlayError>;

    /// Run every command posted since the last drain.
    fn drain(&mut self);

    /// An executor took the player: `waker` tells it when the player's
    /// handles post a command. Commands posted while no executor held the
    /// player woke no one, so the executor drains them once it holds it.
    fn hold(&mut self, waker: Waker);

    /// The executor let the player go: later commands wake no one and wait
    /// for the next executor to hold it.
    fn release(&mut self);

    /// Advance control-plane and audio-backend work.
    fn tick(&mut self) -> Result<(), PlayError>;
}

/// Produces a cloneable command capability without sharing player identity.
pub trait PlayerControlSource: Player {
    /// Concrete command capability retained by typed host-owned handles.
    type Control: Clone + MaybeSend + MaybeSync + 'static;

    /// Typed pool schema shared with the canonical playback session.
    type Schema;

    /// Attaches the resident Player to its canonical session exactly once and
    /// returns what its deck registers with there.
    fn attach_session(
        &mut self,
        binding: SessionBinding<Self::Schema>,
    ) -> Result<DeckRegistration<Self::Schema>, PlayError>;

    /// Closes the resident player through a previously issued capability.
    fn close_control(control: &Self::Control) -> Result<(), PlayError>;

    /// Creates a command capability for this player.
    fn control(&self) -> Self::Control;

    /// Takes the slot the session built for the deck it registered.
    fn seat(&mut self, slot: AllocatedSlot);
}

/// A bare player runs its control's commands on the caller, behind its own
/// operations gate, so an executor that holds it has nothing to drain.
impl<S> Player for PlayerImpl<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn close(&mut self) -> Result<(), PlayError> {
        self.make_control().close()
    }

    fn drain(&mut self) {}

    fn hold(&mut self, _waker: Waker) {}

    fn release(&mut self) {}

    fn tick(&mut self) -> Result<(), PlayError> {
        self.runtime.with_open_result(PlayerRuntime::tick)
    }
}

impl<S> PlayerControlSource for PlayerImpl<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    type Control = crate::player::PlayerControl<S>;
    type Schema = S;

    fn attach_session(
        &mut self,
        binding: SessionBinding<S>,
    ) -> Result<DeckRegistration<S>, PlayError> {
        self.runtime.attach_session(binding)?;
        Ok(self.runtime.core.engine.registration())
    }

    fn close_control(control: &Self::Control) -> Result<(), PlayError> {
        control.close()
    }

    fn control(&self) -> Self::Control {
        self.make_control()
    }

    fn seat(&mut self, slot: AllocatedSlot) {
        self.runtime.core.engine.seat(slot);
    }
}
