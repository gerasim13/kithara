use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_sync::{LoadGeneration, SourceChange, SourceReservation, SourceRevision};
use ringbuf::traits::{Observer, Producer};

use super::core::EngineImpl;
use crate::{
    api::{CrossfadeSettings, SlotId},
    bridge::{PlayerCmd, TrackTransition},
    error::PlayError,
    rt::track::PlayerResource,
    session::SessionError,
};

/// One change of what this player's resident plays, held from before the
/// audio thread can see it until it is reported. Without a Host session no
/// owner plans against the source, so there is nothing to hold.
#[must_use]
pub(crate) struct SourceEdit(Option<SourceReservation>);

impl SourceEdit {
    /// Report the change once the audio thread holds it. Dropping the edit
    /// instead means nothing changed.
    pub(crate) fn commit(self, change: SourceChange) {
        if let Some(reservation) = self.0 {
            reservation.publish(change);
        }
    }
}

/// Capacity held for one off-RT load while its resource is moved out of the
/// queue. Other producers cannot consume these entries before the load sends.
pub(crate) struct SlotLoadReservation<'a, S> {
    engine: &'a EngineImpl<S>,
    slot: SlotId,
    transition: Option<CrossfadeSettings>,
    count: usize,
    active: bool,
}

impl<S> SlotLoadReservation<'_, S> {
    /// Publish the load and optional `FadeIn` together after preparation.
    /// A `FadeIn` makes the item leading with `duration_seconds`, so the
    /// playhead reads describe it from the send on.
    pub(crate) fn send(
        mut self,
        item_id: TrackId,
        load: LoadGeneration,
        resource: Box<PlayerResource>,
        duration_seconds: f64,
    ) {
        let mut slots = self.engine.slots.lock();
        let Some(entry) = slots.entry_mut(self.slot) else {
            unreachable!("a slot with reserved commands was released");
        };
        let handle = &mut entry.control;
        let seek = resource.seek_handle();
        let render = resource.render_reader();
        if handle
            .cmd_tx
            .try_push(PlayerCmd::LoadTrack {
                item_id,
                load,
                resource,
            })
            .is_err()
        {
            unreachable!("reserved load command entry disappeared");
        }
        if let Some(settings) = self.transition
            && handle
                .playback
                .lead(duration_seconds, |epoch| {
                    handle
                        .cmd_tx
                        .try_push(PlayerCmd::Transition(TrackTransition::FadeIn {
                            item_id,
                            settings,
                            epoch,
                        }))
                })
                .is_err()
        {
            unreachable!("reserved FadeIn command entry disappeared");
        }
        if let Some(seek) = seek {
            handle.bind_seek(item_id, seek);
        }
        if let Some(render) = render {
            handle.bind_render(item_id, load, render);
        }
        entry.reserved_cmds -= self.count;
        self.active = false;
        drop(slots);
    }
}

impl<S> Drop for SlotLoadReservation<'_, S> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut slots = self.engine.slots.lock();
        if let Some(entry) = slots.entry_mut(self.slot) {
            entry.reserved_cmds -= self.count;
        }
    }
}

impl<S> EngineImpl<S> {
    /// Hold this player's source against Host plans before changing it.
    ///
    /// # Errors
    /// Returns an error when another change of the same source is in flight
    /// or its revision space is spent.
    pub(crate) fn edit_source(&self) -> Result<SourceEdit, PlayError> {
        self.session
            .sync_gate()
            .map(|gate| gate.reserve_source())
            .transpose()
            .map(SourceEdit)
            .map_err(|error| SessionError::SyncControl(error).into())
    }

    /// The source revision whose commands `slot`'s audio callback applied
    /// and rendered, if a Host session arbitrates this player's source.
    pub(crate) fn applied_source(&self, slot: SlotId) -> Option<SourceRevision> {
        self.slot_playback(slot)?.applied_source.load()
    }

    /// Admit the seek before changing any reader. The slots lock owns the sole
    /// command producer, so a free entry cannot disappear before the push.
    pub(crate) fn send_slot_seek(
        &self,
        slot: SlotId,
        position: Duration,
        seconds: f64,
    ) -> Result<(), PlayError> {
        let mut slots = self.slots.lock();
        let entry = slots.entry_mut(slot).ok_or(PlayError::SlotNotFound(slot))?;
        if entry.closing {
            return Err(PlayError::SlotBusy { slot });
        }
        if entry.control.cmd_tx.vacant_len() <= entry.reserved_cmds {
            return Err(PlayError::SlotChannelFull { slot });
        }
        let handle = &mut entry.control;
        let seek_epoch = handle.playback.next_seek_epoch();
        handle.begin_seek(position);
        // The audio consumer only increases vacancy while this lock excludes
        // other producers, so this push cannot fail after the vacancy check.
        if handle
            .cmd_tx
            .try_push(PlayerCmd::Seek {
                seek_epoch,
                seconds,
            })
            .is_err()
        {
            unreachable!("reserved seek command entry disappeared");
        }
        drop(slots);
        Ok(())
    }

    /// Reserve command capacity before taking a resource out of the queue.
    /// The prepared resource can be moved into the audio thread without a
    /// second fallible send or an intervening producer stealing `FadeIn` space.
    pub(crate) fn reserve_slot_load(
        &self,
        slot: SlotId,
        transition: Option<CrossfadeSettings>,
    ) -> Result<SlotLoadReservation<'_, S>, PlayError> {
        let count = usize::from(transition.is_some()) + 1;
        let mut slots = self.slots.lock();
        let entry = slots.entry_mut(slot).ok_or(PlayError::SlotNotFound(slot))?;
        if entry.closing {
            return Err(PlayError::SlotBusy { slot });
        }
        if entry
            .control
            .cmd_tx
            .vacant_len()
            .saturating_sub(entry.reserved_cmds)
            < count
        {
            return Err(PlayError::SlotChannelFull { slot });
        }
        entry.reserved_cmds += count;
        drop(slots);
        Ok(SlotLoadReservation {
            engine: self,
            slot,
            transition,
            count,
            active: true,
        })
    }

    pub(crate) fn send_slot_cmd(&self, slot: SlotId, cmd: PlayerCmd) -> Result<(), PlayError> {
        if matches!(cmd, PlayerCmd::LoadTrack { .. }) {
            return Err(PlayError::Internal(
                "load requires command reservation".into(),
            ));
        }
        let mut slots = self.slots.lock();
        let result = match slots.entry_mut(slot) {
            Some(entry) => {
                if entry.closing {
                    Err(PlayError::SlotBusy { slot })
                } else if entry.control.cmd_tx.vacant_len() <= entry.reserved_cmds {
                    Err(PlayError::SlotChannelFull { slot })
                } else {
                    let breaks = cmd.breaks_sync_plans();
                    let pushed = entry
                        .control
                        .cmd_tx
                        .try_push(cmd)
                        .map_err(|_| PlayError::SlotChannelFull { slot });
                    if breaks && pushed.is_ok() {
                        entry.control.leave_sync_plans();
                    }
                    pushed
                }
            }
            None => Err(PlayError::SlotNotFound(slot)),
        };
        drop(slots);
        result
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;
    use kithara_warp::{RenderPublisher, WarpMapRevision, WarpPlanSlot};

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig, mock,
        player::{PlayerConfig, PlayerImpl},
        test_pools::pools,
    };

    #[kithara::test]
    #[case::pause(PlayerCmd::SetPaused(true), true)]
    #[case::resume(PlayerCmd::SetPaused(false), true)]
    #[case::fade(PlayerCmd::SetFadeDuration(1.0), false)]
    fn a_pause_or_resume_takes_the_slot_sync_lanes_off_their_plans(
        #[case] cmd: PlayerCmd,
        #[case] leaves: bool,
    ) {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .session(mock::session())
                .build(),
        );
        player.play();
        let slot = player.slot().expect("play allocates a slot");
        let engine = player.engine();
        let lane = Arc::new(WarpPlanSlot::default());
        lane.install(Some(Arc::new(mock::entering_plan())));
        engine
            .slots
            .lock()
            .entry_mut(slot)
            .expect("allocated slot")
            .control
            .bind_sync_resource(
                TrackId::allocate(),
                LoadGeneration::first(),
                WarpMapRevision::first(),
                None,
                RenderPublisher::default().reader(),
                Some(Arc::clone(&lane)),
            );

        engine
            .send_slot_cmd(slot, cmd)
            .expect("slot takes the command");

        assert_eq!(lane.load().is_none(), leaves);
    }
}
