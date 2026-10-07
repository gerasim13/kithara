use kithara_bufpool::HasPool;
use kithara_command::Refused;
use kithara_events::TrackId;
use kithara_play::{CrossfadeSettings, EqBandConfig, InterruptionKind, PlayError};

use super::{
    QueueControl, Transition,
    command::{PlayerCall, QueueCommand},
};
use crate::{
    error::QueueError,
    navigation::{ActionAtItemEnd, PlaybackOrder, RepeatMode},
    track::TrackSource,
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Append a track. Loading starts immediately in the background.
    /// The id is allocated from the global counter via
    /// [`TrackId::allocate`]; use [`Self::append_with_id`] when the
    /// caller owns the id (FFI item pre-allocation).
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append<T: Into<TrackSource<S>>>(&self, source: T) -> Result<TrackId, QueueError> {
        self.append_with_id(TrackId::allocate(), source)
    }

    /// Append a track with a caller-supplied id. The id MUST come from
    /// [`TrackId::allocate`] so it stays inside the process-wide
    /// monotonic address space. Used by the FFI layer where the item
    /// reserves its id at construction and surfaces it as `audioId`
    /// before insert.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append_with_id<T: Into<TrackSource<S>>>(
        &self,
        id: TrackId,
        source: T,
    ) -> Result<TrackId, QueueError> {
        let source = source.into();
        self.call(QueueCommand::Append { id, source })?;
        Ok(id)
    }

    /// Remove all tracks from the queue. Dropping the records aborts
    /// their in-flight loads.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed, and
    /// the deck's refusal to clear; the queue keeps its tracks then.
    pub fn clear(&self) -> Result<(), QueueError> {
        self.call(QueueCommand::Clear)
    }

    /// Close the resident player, then irreversibly cancel queue-owned work.
    ///
    /// # Errors
    ///
    /// Returns the player detach failure without cancelling the queue token,
    /// so the owner can retry.
    pub fn close(&self) -> Result<(), QueueError> {
        self.call(QueueCommand::Close)
    }

    /// Insert a track after the given id, or at the head when `after` is
    /// `None`. Loading starts immediately.
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `after` does not match any
    /// track.
    pub fn insert<T: Into<TrackSource<S>>>(
        &self,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        self.insert_with_id(TrackId::allocate(), source, after)
    }

    /// Insert a track with a caller-supplied id. See
    /// [`Self::append_with_id`] for why the id MUST come from
    /// [`TrackId::allocate`].
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `after` does not match
    /// any track.
    pub fn insert_with_id<T: Into<TrackSource<S>>>(
        &self,
        id: TrackId,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        let source = source.into();
        self.call(QueueCommand::Insert { id, source, after })?;
        Ok(id)
    }

    /// Advance to the next track per navigation rules; the queue stays where
    /// it is when it has ended (and
    /// [`RepeatMode::Off`](crate::navigation::RepeatMode::Off) is active).
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the successor cannot be selected.
    pub fn next(&self, transition: Transition) -> Result<(), QueueError> {
        self.call(QueueCommand::Next(transition))
    }

    /// The platform interrupted, or released, the audio output.
    ///
    /// Recording the fact is all this does: an interruption leaves the native
    /// output unscheduled, and restoring it is the route-invalidation path.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        let _ = self.call_player(PlayerCall::NotifyInterruption(kind));
    }

    /// Pause playback and freeze the queue-visible head position.
    pub fn pause(&self) {
        let _ = self.call(QueueCommand::Pause);
    }

    /// Starts what the deck holds, handing it the loaded track it lacks or
    /// retaining the selection until loading finishes.
    pub fn play(&self) {
        let _ = self.call(QueueCommand::Play);
    }

    /// Go back to the previous track; the queue stays where it is at index 0.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the predecessor cannot be selected.
    pub fn previous(&self, transition: Transition) -> Result<(), QueueError> {
        self.call(QueueCommand::Previous(transition))
    }

    /// Remove a track from the queue by id.
    ///
    /// If the removed track is currently playing:
    /// - with tracks remaining → switches to the next (or previous if
    ///   we were at the tail) with an immediate cut.
    /// - with no tracks remaining → pauses the player.
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `id` is not in the queue.
    pub fn remove(&self, id: TrackId) -> Result<(), QueueError> {
        self.call(QueueCommand::Remove(id))
    }

    /// Reset all EQ bands to 0 dB.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal.
    pub fn reset_eq(&self) -> Result<(), QueueError> {
        self.call_player(PlayerCall::ResetEq)
    }

    /// Seek within the currently-playing track.
    ///
    /// Seek-hang detection is not handled here: the audio pipeline's
    /// own `#[hang_watchdog]` instrumentation (e.g. `Audio::read`,
    /// `Stream::read`, `decode_next_chunk`) already panics with a
    /// stacktrace and context dump when no progress is observed. Adding
    /// a second Queue-level watchdog would just duplicate those panics.
    ///
    /// The landed position is reconciled by the worker after it applies the
    /// seek; a target beyond the known track duration is accepted and lands
    /// at the end.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] if the player reports a seek failure.
    pub fn seek(&self, seconds: f64) -> Result<(), QueueError> {
        self.call(QueueCommand::Seek(seconds))
    }

    /// Select a track by id, applying the given [`Transition`]. If the
    /// track is still loading or pending, both the id and the
    /// transition are stashed and applied when loading finishes.
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `id` is not in the queue,
    /// [`QueueError::NotReady`] if the track is in a terminal failed state,
    /// or [`QueueError::Play`] if the deck refuses the selection.
    pub fn select(&self, id: TrackId, transition: Transition) -> Result<(), QueueError> {
        self.call(QueueCommand::Select { id, transition })
    }

    pub fn set_action_at_item_end(&self, action: ActionAtItemEnd) {
        let _ = self.call(QueueCommand::SetActionAtItemEnd(action));
    }

    /// Update the profile captured by future transitions. A successor armed
    /// for the other kind of link comes off the deck, to be armed again for
    /// this one.
    ///
    /// # Errors
    /// Returns an error when any profile value is invalid.
    pub fn set_crossfade_settings(&self, settings: CrossfadeSettings) -> Result<(), QueueError> {
        self.call(QueueCommand::SetCrossfadeSettings(settings))
    }

    /// Set the default playback rate.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal:
    /// [`PlayError::InvalidParameter`] for a rate that is not a finite number,
    /// or the deck's refusal of the new rate.
    pub fn set_default_rate(&self, rate: f32) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetDefaultRate(rate))
    }

    /// Set gain for an EQ band.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetEqGain { band, gain_db })
    }

    /// Replace the live player's EQ band layout.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetEqLayout(layout))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its volume.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal: [`PlayError::MixLevel`] for a level
    /// outside `0.0..=1.0`, or the deck's refusal of the change.
    pub fn set_level(&self, level: f32) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetLevel(level))
    }

    /// Set the mute flag.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal.
    pub fn set_muted(&self, muted: bool) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetMuted(muted))
    }

    pub fn set_playback_order(&self, order: PlaybackOrder) {
        let _ = self.call(QueueCommand::SetPlaybackOrder(order));
    }

    /// Set the live playback rate (mirrors into the tempo-mode sibling
    /// so a running key-locked stretch tracks the move).
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal:
    /// [`PlayError::InvalidParameter`] for a rate that is not a finite number,
    /// or the lanes' or the deck's refusal of the new rate.
    pub fn set_rate(&self, rate: f32) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetRate(rate))
    }

    /// Set repeat mode.
    pub fn set_repeat(&self, mode: RepeatMode) {
        let _ = self.call(QueueCommand::SetRepeat(mode));
    }

    /// Replace the entire queue with the given sources.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed, and
    /// the deck's refusal to clear; the queue keeps its tracks then.
    pub fn set_tracks<I, T>(&self, sources: I) -> Result<(), QueueError>
    where
        I: IntoIterator<Item = T>,
        T: Into<TrackSource<S>>,
    {
        let sources = sources.into_iter().map(Into::into).collect();
        self.call(QueueCommand::SetTracks(sources))
    }

    /// Set the volume (0.0..=1.0).
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with the underlying player's refusal.
    pub fn set_volume(&self, volume: f32) -> Result<(), QueueError> {
        self.call_player(PlayerCall::SetVolume(volume))
    }

    /// Periodic tick: drives `PlayerImpl::tick` and drains queued engine
    /// events: the cursor follows `CurrentItemChanged` to an item the
    /// deck led on to, which is forwarded as
    /// [`QueueEvent::CurrentTrackChanged`](crate::event::QueueEvent::CurrentTrackChanged),
    /// and `ItemDidPlayToEnd` (filtered) acts where the deck led nowhere.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] with `PlayerImpl::tick`'s failure.
    pub fn tick(&self) -> Result<(), QueueError> {
        self.call(QueueCommand::Tick)
    }

    /// Posts `command` and waits for the queue's answer; a queue that was
    /// dropped answers nothing, which reads as closed.
    fn call(&self, command: QueueCommand<S>) -> Result<(), QueueError> {
        let ticket = self.postbox.post(command).map_err(|_| PlayError::Closed)?;
        ticket.wait().map_err(|refused| match refused {
            Refused::Owner(error) => error,
            Refused::Unanswered => PlayError::Closed.into(),
        })
    }

    fn call_player(&self, call: PlayerCall) -> Result<(), QueueError> {
        self.call(QueueCommand::Player(call))
    }
}
