use std::sync::atomic::Ordering;

use kithara_audio::SeekOutcome;
use kithara_bufpool::HasPool;
use kithara_platform::time::Duration;
use kithara_sync::{LoadGeneration, SourceChange};
use tracing::{debug, warn};

use super::super::core::PlayerRuntime;
use crate::{
    api::{CrossfadeSettings, PlayerStatus, SelectionPlayback, TrackId},
    bridge::{PlayerCmd, TrackTransition},
    error::PlayError,
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
            let _ = self.send_paused(false);
            self.enter_playing();
            self.set_status(PlayerStatus::ReadyToPlay);
        } else {
            let _ = self.send_paused(true);
            self.enter_paused();
        }
    }

    /// Stop or restart the resident at a frame no sync plan predicted.
    fn send_paused(&self, paused: bool) -> Result<(), PlayError> {
        self.send_source_change(PlayerCmd::SetPaused(paused), SourceChange::Discontinuity)
    }

    /// Send a command that changes what the resident plays and report the
    /// change once the audio thread holds it.
    pub(crate) fn send_source_change(
        &self,
        cmd: PlayerCmd,
        change: SourceChange,
    ) -> Result<(), PlayError> {
        let edit = self.core.engine.edit_source()?;
        self.send_to_slot(cmd)?;
        edit.commit(change);
        Ok(())
    }

    /// Place the freshly-loaded track at the position handed over before it
    /// existed. Must follow [`Self::start_resident`]: a fade-in re-bases a
    /// track that is past its head, which would undo the seek.
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

    /// Load the current queue item into the active slot.
    ///
    /// Takes the resource out of the queue (replacing with `None`), wraps it
    /// in `PlayerResource`, and sends `LoadTrack` + `FadeIn` to the processor.
    ///
    /// `false` means the slot held no resource, so nothing reached the
    /// processor and the item is not current.
    fn load_current_item(&self) -> Result<bool, PlayError> {
        let index = self.current_index();
        self.load_item_with(
            index,
            CrossfadeSettings {
                duration: self.crossfade_duration(),
                ..CrossfadeSettings::default()
            },
        )
    }

    fn load_item_with(
        &self,
        index: usize,
        crossfade: CrossfadeSettings,
    ) -> Result<bool, PlayError> {
        let edit = self.core.engine.edit_source()?;
        let Some((item_id, load, _src, _duration)) =
            self.enqueue_to_processor(index, Some(crossfade))?
        else {
            return Ok(false);
        };
        self.phase.lock().set_resident((item_id, load));
        edit.commit(SourceChange::Discontinuity);
        self.apply_start_position();
        Ok(true)
    }

    /// Pause playback. The effective rate becomes `0.0` when RT applies the command.
    pub fn pause(&self) {
        let _ = self.send_paused(true);
        self.enter_paused();
        debug!(phase = ?self.phase_kind(), "pause");
    }

    /// Start playback from the configured default-rate target.
    ///
    /// Announces the current item only once a slot is loaded; announcing while the load is still in
    /// flight would mark the index current early and make a later select skip re-enqueuing the
    /// arriving resource.
    pub fn play(&self) {
        let rate = self.core.warp.stretch().speed();

        if let Err(e) = self.ensure_engine_started() {
            warn!(?e, "failed to start engine");
            return;
        }
        if let Err(e) = self.ensure_slot() {
            warn!(?e, "failed to allocate slot");
            return;
        }

        let _ = self.send_to_slot(PlayerCmd::SetFadeDuration(self.crossfade_duration()));
        let _ = self.send_to_slot(PlayerCmd::SetPrefetchDuration(self.prefetch_duration()));
        let loaded = match self.load_current_item() {
            Ok(loaded) => loaded,
            Err(error) => {
                warn!(%error, "failed to start track playback");
                return;
            }
        };
        let _ = self.send_paused(false);

        self.enter_playing();
        self.set_status(PlayerStatus::ReadyToPlay);
        if loaded {
            self.announce_current_item(self.current_index());
        }
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

        let edit = self.core.engine.edit_source()?;
        self.core
            .engine
            .send_slot_seek(slot_id, target, target_secs)?;
        edit.commit(SourceChange::Discontinuity);

        if matches!(outcome, SeekOutcome::Landed { .. }) {
            playback.position.store(target_secs, Ordering::Relaxed);
        }

        Ok(outcome)
    }

    /// Select and load a queue item by index, using the configured
    /// crossfade duration for the transition.
    pub fn select_item(&self, index: usize, playback: SelectionPlayback) -> Result<(), PlayError> {
        self.select_item_with_crossfade(
            index,
            SelectTransition {
                playback,
                crossfade: CrossfadeSettings {
                    duration: self.crossfade_duration(),
                    ..CrossfadeSettings::default()
                },
            },
        )
    }

    /// Select and load a queue item by index, applying an explicit
    /// crossfade duration for this one transition only.
    ///
    /// Does not mutate the player-configured crossfade — subsequent
    /// calls to [`select_item`](Self::select_item) fall back to
    /// [`crossfade_duration`](Self::crossfade_duration). Pass `0.0` for an
    /// immediate cut (no fade); matches `AVQueuePlayer`'s manual-selection
    /// idiom.
    ///
    /// Reselecting the already-current item is valid even though its resource was consumed by the
    /// load that made it current: the resource now lives in the processor as the playing track.
    pub fn select_item_with_crossfade(
        &self,
        index: usize,
        transition: SelectTransition,
    ) -> Result<(), PlayError> {
        let SelectTransition {
            playback,
            crossfade,
        } = transition;
        let crossfade = crossfade.validate()?;
        let items_len = self.item_count();
        if index >= items_len {
            return Err(PlayError::IndexOutOfRange {
                index,
                len: items_len,
            });
        }

        let reselecting_current =
            index == self.core.items.current_index() && self.core.items.is_announced(index);
        let has_resource = self.core.items.has_resource(index);

        let armed_for_index = self
            .phase
            .lock()
            .pending()
            .is_some_and(|p| !p.state.activated() && p.index == index);
        if !armed_for_index && !reselecting_current && !has_resource {
            return Err(PlayError::ItemConsumed { index });
        }

        self.ensure_engine_started()?;
        self.ensure_slot()?;

        let _ = self.send_to_slot(PlayerCmd::SetPrefetchDuration(self.prefetch_duration()));

        if armed_for_index {
            self.commit_next(index)?;
        } else if !reselecting_current {
            self.unarm_next_internal(Some(index));
            self.load_item_with(index, crossfade)?;
            self.core.items.set_current(index);
            self.announce_current_item(index);
        }

        self.apply_playback(playback);
        Ok(())
    }

    /// Fade a loaded track in as the resident. The resident is recorded
    /// before the change is reported, so evidence rendered on the new source
    /// never pairs with the track it replaced. Once the processor accepts the
    /// `FadeIn`, the playhead reads describe the new resident at its head with
    /// `duration_seconds` until the audio thread takes it on.
    pub(crate) fn start_resident(
        &self,
        item_id: TrackId,
        load: LoadGeneration,
        duration_seconds: f64,
    ) -> Result<(), PlayError> {
        let edit = self.core.engine.edit_source()?;
        let playback = self
            .slot()
            .and_then(|slot| self.core.engine.slot_playback(slot))
            .ok_or(PlayError::NoActiveSlot)?;
        let settings = CrossfadeSettings {
            duration: self.crossfade_duration(),
            ..CrossfadeSettings::default()
        };
        playback.lead(duration_seconds, |epoch| {
            self.send_to_slot(PlayerCmd::Transition(TrackTransition::FadeIn {
                item_id,
                settings,
                epoch,
            }))
        })?;
        self.phase.lock().set_resident((item_id, load));
        edit.commit(SourceChange::Discontinuity);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kithara_sync::{SourceChange, mock::MemberOwner};
    use kithara_test_utils::kithara;
    use kithara_warp::BeatGridId;

    use crate::{
        PlayWorker, PlayWorkerConfig,
        bridge::PlayerCmd,
        error::PlayError,
        mock,
        player::{Player, PlayerConfig, PlayerImpl},
        test_pools::{TestPools, pools},
    };

    fn gated_player() -> (PlayerImpl<TestPools>, MemberOwner) {
        let owner = MemberOwner::new(BeatGridId::allocate().expect("member id"));
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .session(mock::session().with_sync_gate(owner.gate()))
                .build(),
        );
        (player, owner)
    }

    /// Reconcile what the player reported so far, as the Host does.
    fn reconcile(owner: &MemberOwner) -> Option<SourceChange> {
        owner.reconcile().expect("owner enters")
    }

    #[kithara::test]
    fn a_seek_the_audio_thread_holds_reports_a_discontinuity() {
        let (player, owner) = gated_player();
        player.play();
        assert_eq!(
            reconcile(&owner),
            Some(SourceChange::Discontinuity),
            "resuming restarts the source at an unplanned frame"
        );
        let before = owner.gate().source_revision();

        Player::seek_seconds(&player, 1.0).expect("the slot admits the seek");

        assert_ne!(owner.gate().source_revision(), before);
        assert_eq!(reconcile(&owner), Some(SourceChange::Discontinuity));
        drop(
            owner
                .gate()
                .reserve_source()
                .expect("the committed seek released the source"),
        );
    }

    #[kithara::test]
    fn a_seek_the_audio_thread_cannot_hold_changes_nothing() {
        let (player, owner) = gated_player();
        player.play();
        let _ = reconcile(&owner);
        while player.send_to_slot(PlayerCmd::SetFadeDuration(0.0)).is_ok() {}
        let before = owner.gate().source_revision();

        assert!(matches!(
            Player::seek_seconds(&player, 1.0),
            Err(PlayError::SlotChannelFull { .. })
        ));

        assert_eq!(owner.gate().source_revision(), before);
        assert_eq!(reconcile(&owner), None);
        drop(
            owner
                .gate()
                .reserve_source()
                .expect("the refused seek released the source"),
        );
    }

    #[kithara::test]
    fn pause_and_speed_report_their_change() {
        let (player, owner) = gated_player();
        player.play();
        let _ = reconcile(&owner);

        player.set_rate(1.5);
        assert_eq!(reconcile(&owner), Some(SourceChange::Timing));
        player.pause();
        assert_eq!(reconcile(&owner), Some(SourceChange::Discontinuity));
        player.set_default_rate(0.8);
        assert_eq!(
            reconcile(&owner),
            Some(SourceChange::Timing),
            "a paused rate change still moves where playback resumes"
        );
    }

    #[kithara::test]
    fn a_closed_player_seeks_without_touching_the_source() {
        let (mut player, owner) = gated_player();
        player.play();
        let _ = reconcile(&owner);
        Player::close(&mut player).expect("close");
        let before = owner.gate().source_revision();

        assert!(matches!(
            Player::seek_seconds(&player, 1.0),
            Err(PlayError::Closed)
        ));

        assert_eq!(owner.gate().source_revision(), before);
        assert_eq!(reconcile(&owner), None);
    }
}
