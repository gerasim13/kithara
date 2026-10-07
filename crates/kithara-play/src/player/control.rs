use core::ops::Deref;

use kithara_command::Refused;
use kithara_events::TrackId;

use super::{PlayerCommand, PlayerView, SelectTransition, command::PlayerPostbox};
use crate::{
    EqBandConfig, InterruptionKind, PlayError, Resource, SelectionPlayback, SuccessorLink,
};

/// A handle on one player: it posts commands for the executor that holds the
/// player and reads what the player last published.
///
/// Every command waits for its answer, which the holder gives once it drains
/// the player; a command posted before any executor holds the player waits
/// for one, so a command must never be called on the thread that holds the
/// player. Once the player is dropped a command fails with
/// [`PlayError::Closed`].
#[derive(Clone)]
pub struct PlayerControl {
    postbox: PlayerPostbox,
    view: PlayerView,
}

impl Deref for PlayerControl {
    type Target = PlayerView;

    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl PlayerControl {
    pub(super) const fn new(postbox: PlayerPostbox, view: PlayerView) -> Self {
        Self { postbox, view }
    }

    /// Attach `resource` to the deck as `item`, ahead of the current item and
    /// joined to it by `link`.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, the failure to allocate its
    /// buffers, or the deck's refusal for want of room. Nothing is armed
    /// then, and the resource is spent: the item must be loaded again.
    pub fn arm_next(
        &self,
        item: TrackId,
        resource: Resource,
        link: SuccessorLink,
    ) -> Result<(), PlayError> {
        self.call(PlayerCommand::ArmNext {
            item,
            resource,
            link,
        })
    }

    /// Silences the deck, then closes the player.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal of the clear;
    /// the player stays open then.
    pub fn close(&self) -> Result<(), PlayError> {
        self.call(PlayerCommand::Close)
    }

    /// Drop the armed successor from the deck without committing it.
    pub fn unarm_next(&self) {
        self.command(PlayerCommand::UnarmNext);
    }

    /// Record that the platform interrupted, or released, the audio output.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        self.command(PlayerCommand::NotifyInterruption(kind));
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.command(PlayerCommand::Pause);
    }

    /// Start or resume playback.
    pub fn play(&self) {
        self.command(PlayerCommand::Play);
    }

    /// Drop every track the deck holds and release its slot.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal of the clear;
    /// the player keeps its tracks then.
    pub fn remove_all_items(&self) -> Result<(), PlayError> {
        self.call(PlayerCommand::RemoveAllItems)
    }

    /// Reset all EQ bands.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
        self.call(PlayerCommand::ResetEq)
    }

    /// Seek within the current item; where it landed is read from the view
    /// once the seek is answered.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal of the seek.
    pub fn seek_seconds(&self, seconds: f64) -> Result<(), PlayError> {
        self.call(PlayerCommand::Seek(seconds))
    }

    /// Make `item` current with the configured crossfade.
    ///
    /// # Errors
    /// As [`Self::select_with_crossfade`].
    pub fn select(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        playback: SelectionPlayback,
    ) -> Result<(), PlayError> {
        self.call(PlayerCommand::Select {
            item,
            resource,
            playback,
        })
    }

    /// Make `item` current: a given `resource` loads as `item`; without one
    /// the deck commits its armed successor `item` or reselects its current
    /// item.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::ItemConsumed`] when the
    /// deck holds no `item` and no resource came, or the load's failure. The
    /// resource is spent on any error.
    pub fn select_with_crossfade(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        transition: SelectTransition,
    ) -> Result<(), PlayError> {
        self.call(PlayerCommand::SelectWithCrossfade {
            item,
            resource,
            transition,
        })
    }

    /// Set the crossfade duration later selections use.
    pub fn set_crossfade_duration(&self, seconds: f32) {
        self.command(PlayerCommand::SetCrossfadeDuration(seconds));
    }

    /// Set the default playback rate.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::InvalidParameter`] for
    /// a rate that is not a finite number, or the deck's refusal of the rate.
    pub fn set_default_rate(&self, rate: f32) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetDefaultRate(rate))
    }

    /// Set one EQ band's gain.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the band's or the deck's refusal.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetEqGain { band, gain_db })
    }

    /// Replace the EQ band layout.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the layout's or the deck's
    /// refusal.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetEqLayout(layout))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its
    /// volume.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::MixLevel`] for a level
    /// outside `0.0..=1.0`, or the deck's refusal.
    pub fn set_level(&self, level: f32) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetLevel(level))
    }

    /// Set the mute state.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn set_muted(&self, muted: bool) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetMuted(muted))
    }

    /// Set the live playback rate.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::InvalidParameter`] for
    /// a rate that is not a finite number, or the deck's refusal of the rate.
    pub fn set_rate(&self, rate: f32) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetRate(rate))
    }

    /// Set the output volume.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn set_volume(&self, volume: f32) -> Result<(), PlayError> {
        self.call(PlayerCommand::SetVolume(volume))
    }

    /// Handle what the deck reported since the last tick.
    pub fn tick(&self) {
        self.command(PlayerCommand::Tick);
    }

    /// Posts `command` and waits for the player's answer.
    fn call(&self, command: PlayerCommand) -> Result<(), PlayError> {
        let ticket = self.postbox.post(command).map_err(|_| PlayError::Closed)?;
        ticket.wait().map_err(|refused| match refused {
            Refused::Owner(error) => error,
            Refused::Unanswered => PlayError::Closed,
        })
    }

    /// Posts a command only a closed player refuses, and waits for it.
    fn command(&self, command: PlayerCommand) {
        let _ = self.call(command);
    }
}
