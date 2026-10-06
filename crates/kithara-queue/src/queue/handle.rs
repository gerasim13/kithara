use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_platform::sync::mpsc;
use kithara_play::{CrossfadeSettings, EqBandConfig, InterruptionKind, PlayError, SeekOutcome};

use super::{
    QueueControl, Transition,
    command::{PlayerCall, QueueCommand, Reply},
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
        self.call(|reply| QueueCommand::Append { id, source, reply })?
    }

    /// Remove all tracks from the queue. Dropping the records aborts
    /// their in-flight loads.
    pub fn clear(&self) {
        self.command(QueueCommand::Clear);
    }

    /// Close the resident player, then irreversibly cancel queue-owned work.
    ///
    /// # Errors
    ///
    /// Returns the player detach failure without cancelling the queue token,
    /// so the owner can retry.
    pub fn close(&self) -> Result<(), PlayError> {
        self.call(QueueCommand::Close)?
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
        self.call(|reply| QueueCommand::Insert {
            id,
            source,
            after,
            reply,
        })?
    }

    /// Advance to the next track per navigation rules. Returns the newly
    /// selected id, or `None` when the queue has ended (and
    /// [`RepeatMode::Off`](crate::navigation::RepeatMode::Off) is active).
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the successor cannot be selected.
    pub fn next(&self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
        self.call(|reply| QueueCommand::Next { transition, reply })?
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
        self.command(QueueCommand::Pause);
    }

    /// Starts what the deck holds, handing it the loaded track it lacks or
    /// retaining the selection until loading finishes.
    pub fn play(&self) {
        self.command(QueueCommand::Play);
    }

    /// Go back to the previous track. Returns the newly selected id, or
    /// `None` at index 0.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the predecessor cannot be selected.
    pub fn previous(&self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
        self.call(|reply| QueueCommand::Previous { transition, reply })?
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
        self.call(|reply| QueueCommand::Remove { id, reply })?
    }

    /// Reset all EQ bands to 0 dB.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
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
    /// Returns the typed [`SeekOutcome`] — either `Landed` with the
    /// requested target (the actual landed position is reconciled by the
    /// worker after applying the seek; this call returns the optimistic
    /// outcome) or `PastEof` if the target is beyond the known track
    /// duration.
    ///
    /// # Errors
    /// Returns [`QueueError::Play`] if the player reports a seek failure.
    pub fn seek(&self, seconds: f64) -> Result<SeekOutcome, QueueError> {
        self.call(|reply| QueueCommand::Seek { seconds, reply })?
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
        self.call(|reply| QueueCommand::Select {
            id,
            transition,
            reply,
        })?
    }

    pub fn set_action_at_item_end(&self, action: ActionAtItemEnd) {
        self.command(|reply| QueueCommand::SetActionAtItemEnd { action, reply });
    }

    /// Update the profile captured by future transitions. A successor armed
    /// for the other kind of link comes off the deck, to be armed again for
    /// this one.
    ///
    /// # Errors
    /// Returns an error when any profile value is invalid.
    pub fn set_crossfade_settings(&self, settings: CrossfadeSettings) -> Result<(), PlayError> {
        self.call(|reply| QueueCommand::SetCrossfadeSettings { settings, reply })?
    }

    /// Set the default playback rate.
    pub fn set_default_rate(&self, rate: f32) {
        let _ = self.call_player(PlayerCall::SetDefaultRate(rate));
    }

    /// Set gain for an EQ band.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.call_player(PlayerCall::SetEqGain { band, gain_db })
    }

    /// Replace the live player's EQ band layout.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError> {
        self.call_player(PlayerCall::SetEqLayout(layout))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its volume.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player: [`PlayError::MixLevel`] for a level
    /// outside `0.0..=1.0`, or the deck's refusal of the change.
    pub fn set_level(&self, level: f32) -> Result<(), PlayError> {
        self.call_player(PlayerCall::SetLevel(level))
    }

    /// Set the mute flag.
    pub fn set_muted(&self, muted: bool) {
        let _ = self.call_player(PlayerCall::SetMuted(muted));
    }

    pub fn set_playback_order(&self, order: PlaybackOrder) {
        self.command(|reply| QueueCommand::SetPlaybackOrder { order, reply });
    }

    /// Set the live playback rate (mirrors into the tempo-mode sibling
    /// so a running key-locked stretch tracks the move).
    pub fn set_rate(&self, rate: f32) {
        let _ = self.call_player(PlayerCall::SetRate(rate));
    }

    /// Set repeat mode.
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.command(|reply| QueueCommand::SetRepeat { mode, reply });
    }

    /// Replace the entire queue with the given sources.
    pub fn set_tracks<I, T>(&self, sources: I)
    where
        I: IntoIterator<Item = T>,
        T: Into<TrackSource<S>>,
    {
        let sources = sources.into_iter().map(Into::into).collect();
        self.command(|reply| QueueCommand::SetTracks { sources, reply });
    }

    /// Set the volume (0.0..=1.0).
    pub fn set_volume(&self, volume: f32) {
        let _ = self.call_player(PlayerCall::SetVolume(volume));
    }

    /// Periodic tick: drives `PlayerImpl::tick` and drains queued engine
    /// events: the cursor follows `CurrentItemChanged` to an item the
    /// deck led on to, which is forwarded as
    /// [`QueueEvent::CurrentTrackChanged`](crate::event::QueueEvent::CurrentTrackChanged),
    /// and `ItemDidPlayToEnd` (filtered) acts where the deck led nowhere.
    ///
    /// # Errors
    /// Forwards `PlayError` from `PlayerImpl::tick`.
    pub fn tick(&self) -> Result<(), QueueError> {
        self.call(QueueCommand::Tick)?
    }

    pub(super) fn prepare(&self) -> Result<(), PlayError> {
        self.call(QueueCommand::Prepare)?
    }

    /// Posts the command `command` builds around its reply and waits for the
    /// answer; a queue that was dropped answers nothing.
    fn call<T>(&self, command: impl FnOnce(Reply<T>) -> QueueCommand<S>) -> Result<T, PlayError> {
        let (reply, answer) = mpsc::channel();
        self.postbox
            .post(command(reply))
            .map_err(|_| PlayError::Closed)?;
        answer.recv().map_err(|_| PlayError::Closed)
    }

    fn call_player(&self, call: PlayerCall) -> Result<(), PlayError> {
        self.call(|reply| QueueCommand::Player { call, reply })?
    }

    /// Runs `command` on the queue, which does nothing once it is closed.
    fn command(&self, command: impl FnOnce(Reply<()>) -> QueueCommand<S>) {
        let _ = self.call(command);
    }
}
