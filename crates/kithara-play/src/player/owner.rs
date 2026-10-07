use kithara_audio::SeekOutcome;
use kithara_bufpool::HasPool;
use kithara_events::TrackId;

use super::{PlayerCommand, PlayerImpl, PlayerRuntime, SelectTransition};
use crate::{
    EqBandConfig, InterruptionKind, PlayError, Resource, SelectionPlayback, SuccessorLink,
};

/// What the player's owner calls: each command runs on an open player, then
/// the player publishes what it changed for its handles to read.
impl<S> PlayerImpl<S>
where
    S: HasPool<f32>,
{
    /// Runs `command` as the owner. A closed player refuses every command.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the command's own refusal.
    pub fn run(&self, command: PlayerCommand) -> Result<(), PlayError> {
        match command {
            PlayerCommand::ArmNext {
                item,
                resource,
                link,
            } => self.arm_next(item, resource, link),
            PlayerCommand::UnarmNext => self.settle(PlayerRuntime::unarm_next),
            PlayerCommand::NotifyInterruption(kind) => {
                self.settle(|runtime| runtime.notify_interruption(kind))
            }
            PlayerCommand::Pause => self.settle(PlayerRuntime::pause),
            PlayerCommand::Play => self.settle(PlayerRuntime::play),
            PlayerCommand::RemoveAllItems => self.remove_all_items(),
            PlayerCommand::ResetEq => self.reset_eq(),
            PlayerCommand::Seek(seconds) => self.seek_seconds(seconds).map(drop),
            PlayerCommand::Select {
                item,
                resource,
                playback,
            } => self.select(item, resource, playback),
            PlayerCommand::SelectWithCrossfade {
                item,
                resource,
                transition,
            } => self.select_with_crossfade(item, resource, transition),
            PlayerCommand::SetCrossfadeDuration(seconds) => {
                self.settle(|runtime| runtime.set_crossfade_duration(seconds))
            }
            PlayerCommand::SetDefaultRate(rate) => self.set_default_rate(rate),
            PlayerCommand::SetEqGain { band, gain_db } => self.set_eq_gain(band, gain_db),
            PlayerCommand::SetEqLayout(layout) => self.set_eq_layout(layout),
            PlayerCommand::SetLevel(level) => self.set_level(level),
            PlayerCommand::SetMuted(muted) => self.set_muted(muted),
            PlayerCommand::SetRate(rate) => self.set_rate(rate),
            PlayerCommand::SetVolume(volume) => self.set_volume(volume),
            PlayerCommand::Tick => self.settle(PlayerRuntime::process_notifications),
            PlayerCommand::Close => self.close(),
        }
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
        self.owned(|runtime| runtime.arm_next(item, resource, link))
    }

    /// Silences the deck, then closes the player.
    ///
    /// # Errors
    /// The deck's refusal of the clear; the player stays open then.
    pub fn close(&self) -> Result<(), PlayError> {
        let closed = self.runtime.close();
        self.publish();
        closed
    }

    /// Drop the armed successor from the deck without committing it.
    pub fn unarm_next(&self) {
        let _ = self.run(PlayerCommand::UnarmNext);
    }

    /// Record that the platform interrupted, or released, the audio output.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        let _ = self.run(PlayerCommand::NotifyInterruption(kind));
    }

    /// Pause playback.
    pub fn pause(&self) {
        let _ = self.run(PlayerCommand::Pause);
    }

    /// Start or resume playback.
    pub fn play(&self) {
        let _ = self.run(PlayerCommand::Play);
    }

    /// Handle what the deck reported since the last tick.
    pub fn process_notifications(&self) {
        let _ = self.run(PlayerCommand::Tick);
    }

    /// Drop every track the deck holds and release its slot.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal of the
    /// clear; the player keeps its tracks then.
    pub fn remove_all_items(&self) -> Result<(), PlayError> {
        self.owned(PlayerRuntime::remove_all_items)
    }

    /// Reset all EQ bands.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
        self.owned(PlayerRuntime::reset_eq)
    }

    /// Seek within the current item.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal of the seek.
    pub fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        self.owned(|runtime| runtime.seek_seconds(seconds))
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
        self.owned(|runtime| runtime.select(item, resource, playback))
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
        self.owned(|runtime| runtime.select_with_crossfade(item, resource, transition))
    }

    /// Set the crossfade duration later selections use.
    pub fn set_crossfade_duration(&self, seconds: f32) {
        let _ = self.run(PlayerCommand::SetCrossfadeDuration(seconds));
    }

    /// Set the default playback rate.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::InvalidParameter`] for
    /// a rate that is not a finite number, or the deck's refusal of the rate.
    pub fn set_default_rate(&self, rate: f32) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_default_rate(rate))
    }

    /// Set one EQ band's gain.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the band's or the deck's refusal.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_eq_gain(band, gain_db))
    }

    /// Replace the EQ band layout.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the layout's or the deck's
    /// refusal.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_eq_layout(layout))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its
    /// volume.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::MixLevel`] for a level
    /// outside `0.0..=1.0`, or the deck's refusal.
    pub fn set_level(&self, level: f32) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_level(level))
    }

    /// Set the mute state.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn set_muted(&self, muted: bool) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_muted(muted))
    }

    /// Set the live playback rate.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, [`PlayError::InvalidParameter`] for
    /// a rate that is not a finite number, or the deck's refusal of the rate.
    pub fn set_rate(&self, rate: f32) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_rate(rate))
    }

    /// Set the output volume.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or the deck's refusal.
    pub fn set_volume(&self, volume: f32) -> Result<(), PlayError> {
        self.owned(|runtime| runtime.set_volume(volume))
    }

    /// Runs `operation` on the open player, then publishes.
    fn owned<T>(
        &self,
        operation: impl FnOnce(&PlayerRuntime<S>) -> Result<T, PlayError>,
    ) -> Result<T, PlayError> {
        let outcome = self.runtime.with_open_result(operation);
        self.publish();
        outcome
    }

    /// Runs `operation`, which cannot be refused, on the open player, then
    /// publishes.
    fn settle(&self, operation: impl FnOnce(&PlayerRuntime<S>)) -> Result<(), PlayError> {
        self.owned(|runtime| {
            operation(runtime);
            Ok(())
        })
    }
}
