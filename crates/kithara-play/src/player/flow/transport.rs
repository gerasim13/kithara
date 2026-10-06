use kithara_audio::SeekOutcome;
use kithara_bufpool::HasPool;
use kithara_platform::time::Duration;
use kithara_render::bridge::DeckPart;
use tracing::{debug, warn};

use super::super::{
    core::{EnqueuedItem, PlayerRuntime},
    track::TrackCommand,
};
use crate::{
    CrossfadeSettings,
    api::{PlayerStatus, SelectionPlayback, TrackId},
    error::PlayError,
    resource::Resource,
};

/// Captured transport intent and complete fade profile for one selection.
#[derive(Debug, Clone, Copy)]
pub struct SelectTransition {
    pub crossfade: CrossfadeSettings,
    pub playback: SelectionPlayback,
}

impl<S> PlayerRuntime<S>
where
    S: HasPool<f32>,
{
    fn apply_playback(&self, playback: SelectionPlayback) {
        if playback == SelectionPlayback::Play {
            let _ = self.send_to_slot(DeckPart::StartAll);
            self.enter_playing();
            self.set_status(PlayerStatus::ReadyToPlay);
        } else {
            let _ = self.send_to_slot(DeckPart::StopAll);
            self.enter_paused();
        }
    }

    /// Place the freshly-loaded track at the position handed over before it
    /// existed. Must follow [`Self::start_playback`]: a seek moves only a track
    /// that is fading in or playing.
    fn apply_start_position(&self) {
        let Some(target) = self.core.start_position.lock().take() else {
            return;
        };
        let seconds = target.as_secs_f64();
        if let Err(e) = self.seek_seconds(seconds) {
            warn!(?e, seconds, "start position rejected by the loaded track");
        }
    }

    /// Ensure the audio engine is started.
    pub fn ensure_engine_started(&self) -> Result<(), PlayError> {
        if self.core.engine.is_running() {
            return Ok(());
        }
        match self.core.engine.start() {
            Ok(()) | Err(PlayError::EngineAlreadyRunning) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Hand `resource` to the deck as `item` and make it the leading track
    /// with `crossfade`, starting where a seek held before it existed.
    fn load_current(
        &self,
        item: TrackId,
        resource: Resource,
        crossfade: CrossfadeSettings,
    ) -> Result<(), PlayError> {
        let EnqueuedItem {
            item_id,
            duration_seconds,
            presentation,
        } = self.enqueue_to_processor(item, resource, None)?;
        self.adopt_presentation(item_id, duration_seconds, presentation);
        self.start_playback_with(item_id, duration_seconds, crossfade);
        self.apply_start_position();
        Ok(())
    }

    /// Pause playback. The effective rate becomes `0.0` when RT applies the command.
    pub fn pause(&self) {
        let _ = self.send_to_slot(DeckPart::StopAll);
        self.enter_paused();
        debug!(phase = ?self.phase_kind(), "pause");
    }

    /// Start or resume what the deck holds at the configured default-rate
    /// target. Loads nothing: a selection hands the deck its item.
    pub fn play(&self) {
        let rate = self.core.tracks.lock().next().speed();

        if let Err(e) = self.ensure_engine_started() {
            warn!(?e, "failed to start engine");
            return;
        }
        if let Err(e) = self.ensure_slot() {
            warn!(?e, "failed to allocate slot");
            return;
        }

        let _ = self.send_to_slot(DeckPart::SetRate(rate));
        let _ = self.send_to_slot(DeckPart::StartAll);

        self.enter_playing();
        self.set_status(PlayerStatus::ReadyToPlay);
        debug!(rate, phase = ?self.phase_kind(), "play");
    }

    /// Seek active tracks to position in seconds.
    ///
    /// Returns the typed [`SeekOutcome`] — either `Landed` with the requested
    /// target (the actual landed position is committed asynchronously by the
    /// worker thread; this call returns the optimistic outcome) or `PastEof`
    /// when the target is past the current track's known duration.
    ///
    /// The outcome is classified against the duration observed *before*
    /// `begin_slot_seek` rebases the source. Reading it afterwards judges the
    /// request against a duration the request itself perturbed: the audio
    /// thread can render a block off the rebased source in that window and
    /// republish a shorter `PlaybackShared::duration`, turning an in-range
    /// target into a spurious `PastEof`.
    ///
    /// A seek that arrives before the player holds a slot is kept as the
    /// current item's start position and applied by the load that starts it,
    /// so a position handed over at queue-seeding time is where playback
    /// begins.
    pub fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        let target_secs = seconds.max(0.0);
        let target = Duration::from_secs_f64(target_secs);

        let Some(slot_id) = self.slot() else {
            *self.core.start_position.lock() = Some(target);
            debug!(target_secs, "seek held until a track is loaded");
            return Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            });
        };

        let Some(playback) = self.core.engine.slot_playback(slot_id) else {
            return Err(PlayError::SlotNotFound(slot_id));
        };
        let outcome = match self.duration_seconds() {
            Some(dur) if target_secs >= dur => SeekOutcome::PastEof {
                target,
                duration: Duration::from_secs_f64(dur),
            },
            _ => SeekOutcome::Landed {
                target,
                landed_at: target,
            },
        };

        let seek_epoch = playback.next_seek_epoch();

        self.core.engine.begin_slot_seek(slot_id, target);

        if let Err(err) = self.send_to_slot(DeckPart::Seek {
            seek_epoch,
            seconds: target_secs,
        }) {
            playback.withdraw_seek_epoch(seek_epoch);
            return Err(err);
        }

        if matches!(outcome, SeekOutcome::Landed { .. }) {
            playback.land_seek(target_secs);
        }

        Ok(outcome)
    }

    /// Make `item` the current item with the configured crossfade. See
    /// [`Self::select_with_crossfade`].
    ///
    /// # Errors
    /// As [`Self::select_with_crossfade`].
    pub fn select(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        playback: SelectionPlayback,
    ) -> Result<(), PlayError> {
        self.select_with_crossfade(
            item,
            resource,
            SelectTransition {
                playback,
                crossfade: self.configured_crossfade(),
            },
        )
    }

    /// Make `item` the current item, applying an explicit crossfade for this
    /// one transition only.
    ///
    /// A given `resource` is loaded as `item`, and the armed successor is
    /// withdrawn. Without one the deck must already hold `item`: the armed
    /// successor is committed, and the current item is reselected in place.
    ///
    /// Does not mutate the player-configured crossfade — subsequent calls to
    /// [`select`](Self::select) fall back to
    /// [`crossfade_duration`](Self::crossfade_duration). Pass `0.0` for an
    /// immediate cut (no fade); matches `AVQueuePlayer`'s manual-selection
    /// idiom.
    ///
    /// # Errors
    /// [`PlayError::ItemConsumed`] when no resource came and the deck holds no
    /// `item`, or the failure to start the engine or load the resource. On
    /// any error the resource is spent: the item must be loaded again.
    pub fn select_with_crossfade(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        transition: SelectTransition,
    ) -> Result<(), PlayError> {
        let SelectTransition {
            playback,
            crossfade,
        } = transition;
        let crossfade = crossfade.validate()?;
        let armed = self.armed_next() == Some(item);
        if resource.is_none() && !armed && self.current_item() != Some(item) {
            return Err(PlayError::ItemConsumed { item });
        }

        self.ensure_engine_started()?;
        self.ensure_slot()?;

        let rate = self.core.tracks.lock().next().speed();
        let _ = self.send_to_slot(DeckPart::SetRate(rate));

        match resource {
            Some(resource) => {
                self.unarm_next_internal(Some(item));
                self.load_current(item, resource, crossfade)?;
                self.core.current.announce(item);
            }
            None if armed => self.commit_next(item, crossfade)?,
            None => {}
        }

        self.apply_playback(playback);
        Ok(())
    }

    /// The player's configured crossfade, for transitions nobody gave settings
    /// of their own.
    pub(crate) fn configured_crossfade(&self) -> CrossfadeSettings {
        CrossfadeSettings {
            duration: self.crossfade_duration(),
            ..CrossfadeSettings::default()
        }
    }

    /// Make `item_id` leading: once the processor accepts its `FadeIn`, the playhead reads
    /// describe it, not only once the audio thread has taken it on, and no withdrawn
    /// successor is left in question.
    pub(crate) fn start_playback_with(
        &self,
        item_id: TrackId,
        duration_seconds: f64,
        settings: CrossfadeSettings,
    ) {
        let Some(playback) = self.slot_playback() else {
            return;
        };
        let led = playback.lead(duration_seconds, |epoch| {
            self.with_tracks(|tracks, out| {
                tracks
                    .apply(item_id, TrackCommand::FadeIn { settings, epoch }, out)
                    .map(drop)
            })
        });
        if led.is_ok()
            && let Some(loads) = self.phase.lock().pending_loads_mut()
        {
            loads.clear_withdrawn();
        }
    }
}
