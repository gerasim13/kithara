use kithara_bufpool::HasPool;
use kithara_command::{Seq, When};
use kithara_events::TrackId;
use kithara_play::{
    Outbox, PlayError, Player, PlayerConfig, Position, Slot, Track, TrackCommand, TrackFactory,
    TrackStatus as PlayingStatus,
};

use super::{
    Queue, Transition,
    slots::{Active, Role},
    types::{Placement, extract_track_name},
};
use crate::{
    AdvanceReason, NavigationState, QueueError, QueueEvent, TrackSource, track::TrackRecord,
};

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) fn insert_entry(
        &mut self,
        id: TrackId,
        source: TrackSource<S>,
        placement: Placement,
    ) {
        let record = TrackRecord::new(id, extract_track_name(&source), source);
        let records = self.tracks.records_mut();
        let index = match placement {
            Placement::Append => {
                records.push(record);
                records.len() - 1
            }
            Placement::At(index) => {
                records.insert(index, record);
                index
            }
        };
        self.navigation.insert(id);
        self.announce(QueueEvent::TrackAdded { id, index });
    }

    pub(super) fn autoplay(&mut self, out: &mut Outbox<'_, S>) -> Result<(), QueueError> {
        if self.config.should_autoplay
            && self.current.is_none()
            && self.target.is_none()
            && self.navigation.current().is_none()
        {
            self.next_target(Transition::None, AdvanceReason::InitialLoad, false, out)?;
        }
        Ok(())
    }

    pub(super) fn remove_entry(
        &mut self,
        id: TrackId,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let index = self
            .tracks
            .records()
            .iter()
            .position(|record| record.id == id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let replacement = if self.current == Some(id) {
            self.tracks
                .records()
                .get(index + 1)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| self.tracks.records().get(previous))
                })
                .map(|record| record.id)
        } else {
            None
        };
        if self.target.is_some_and(|target| target.to == id) {
            self.cancel_target(out)?;
        }
        let mut sent = None;
        for active_index in self.active.indices(|active| {
            active.item == id && !(active.role == Role::Current && replacement.is_some())
        }) {
            sent = self.release_track(active_index, out)?.or(sent);
        }
        drop(self.tracks.records_mut().remove(index));
        self.navigation.reconcile(&self.track_ids());
        self.announce(QueueEvent::TrackRemoved { id });
        if let Some(replacement) = replacement {
            return self.request_transition(
                replacement,
                Transition::None,
                AdvanceReason::RemovedCurrent,
                false,
                out,
            );
        }
        self.reap_released();
        Ok(sent)
    }

    pub(super) fn clear_entries(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let sent = self.release_all(out)?;
        self.cancel_target_answers();
        self.target = None;
        let ids = self.track_ids();
        self.tracks.records_mut().clear();
        let repeat = self.navigation.repeat_mode();
        let order = self.navigation.playback_order();
        self.navigation = NavigationState::new(self.navigation.history_limit());
        self.navigation.set_repeat(repeat);
        self.navigation.set_playback_order(order, &[]);
        for id in ids {
            self.announce(QueueEvent::TrackRemoved { id });
        }
        self.reap_released();
        Ok(sent)
    }

    pub(super) fn load_track(
        &mut self,
        id: TrackId,
        role: Role,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        if let Some(index) = self.active.position(|active| {
            active.item == id && matches!(active.role, Role::Incoming { .. } | Role::Preloaded)
        }) {
            let active = self.active.get_mut(index).ok_or(QueueError::NotReady(id))?;
            active.role = role;
            return Ok(active.load);
        }
        let slot = match self.active.free_slot() {
            Some(slot) => slot,
            None => return self.evict_for(id, role, out),
        };
        let active = self.prepare_track(id, slot, role, out)?;
        let seq = active.load;
        self.active.push(active);
        Ok(seq)
    }

    fn prepare_track(
        &mut self,
        id: TrackId,
        slot: Slot,
        role: Role,
        out: &mut Outbox<'_, S>,
    ) -> Result<Active<F::Track>, QueueError> {
        if out.deck_available() == 0 {
            return Err(PlayError::Full("deck").into());
        }
        if out.dispatcher_available() == 0 {
            return Err(PlayError::Full("dispatcher").into());
        }
        let source = self
            .tracks
            .source(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let observer = self
            .tracks
            .observer(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let settings = self
            .current_track()
            .map_or(self.config.track, Track::projected);
        let mut track = self.config.factory.track(PlayerConfig {
            item: id,
            slot,
            settings,
        })?;
        let Some(loader) = &self.loader else {
            todo!(
                "Inject ResourcePrep and the store at queue registration without changing the infallible facade constructor (contract §8.3; skeleton queue construction ruling)"
            )
        };
        let (item, load) = loader.start(id, source, observer)?;
        let seq = track.apply(
            TrackCommand::Load {
                item,
                position: Position::ZERO,
            },
            out,
        )?;
        self.tracks.begin_load(id, load);
        Ok(Active {
            item: id,
            slot,
            track,
            role,
            load: seq,
        })
    }

    fn evict_for(
        &mut self,
        id: TrackId,
        role: Role,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        if let Some(index) = self.active.replacement_index() {
            self.release_track(index, out)?;
            self.reap_released();
            if self.active.replacement_index().is_some() {
                return Err(QueueError::NotReady(id));
            }
        }
        let victim =
            self.active
                .quietest(&self.deck, |_| true)
                .ok_or(PlayError::InvalidConfiguration {
                    reason: "a mixer must have at least one slot".into(),
                })?;
        let slot = self
            .active
            .get(victim)
            .ok_or(QueueError::NotReady(id))?
            .slot;
        let mut replacement = self.prepare_track(id, slot, role, out)?;
        replacement
            .track
            .apply(TrackCommand::Evict { at: When::Next }, out)?;
        let seq = replacement.load;
        self.active.stage(replacement);
        Ok(seq)
    }

    pub(super) fn release_track(
        &mut self,
        index: usize,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        self.check_release(index)?;
        let replacement = self.active.is_replacement(index);
        let active = self.active.get_mut(index).ok_or(PlayError::NoActiveSlot)?;
        let sent = active.track.apply(TrackCommand::Release, out)?;
        let slot = active.slot;
        active.role = Role::Leaving;
        if !replacement {
            self.active.clear_fade(slot);
        }
        Ok(sent)
    }

    fn check_release(&self, index: usize) -> Result<(), PlayError> {
        let active = self.active.get(index).ok_or(PlayError::NoActiveSlot)?;
        if self.active.is_replacement(index)
            && matches!(active.role, Role::Incoming { batch: Some(_) })
        {
            todo!(
                "kithara-render::DeckProtocol cancellation of a pending unattached Replace before releasing its incoming lane, without Detach of the incumbent (contract §8.6 fast-next)"
            );
        }
        Ok(())
    }

    pub(super) fn release_all(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let mut detach = false;
        let mut dispatcher = 0;
        for active in self.active.iter() {
            if active.track.snapshot().as_ref().attached {
                detach = true;
            } else {
                dispatcher += 1;
            }
        }
        if detach && out.deck_available() == 0 {
            return Err(PlayError::Full("deck"));
        }
        if out.dispatcher_available() < dispatcher {
            return Err(PlayError::Full("dispatcher"));
        }
        for index in 0..self.active.len() {
            self.check_release(index)?;
        }
        let mut release = |out: &mut Outbox<'_, S>| {
            for active in self.active.iter_mut() {
                active.track.apply(TrackCommand::Release, out)?;
            }
            Ok::<(), PlayError>(())
        };
        let sent = if detach {
            out.together(When::Next, release)?.1
        } else {
            release(out)?;
            None
        };
        self.cancel_target_answers();
        for active in self.active.iter_mut() {
            active.role = Role::Leaving;
        }
        Ok(sent)
    }

    pub(super) fn close_tracks(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let sent = self.release_all(out)?;
        self.cancel_target_answers();
        self.target = None;
        self.shutdown.cancel();
        self.tracks.cancel_loads();
        self.reap_released();
        Ok(sent)
    }

    pub(super) fn reap_released(&mut self) {
        for index in self
            .active
            .indices(|active| active.track.snapshot().as_ref().status == PlayingStatus::Released)
            .into_iter()
            .rev()
        {
            let active = self.active.remove(index);
            if active.role == Role::Leaving
                && self.current == Some(active.item)
                && self.active_current_index().is_none()
            {
                self.current = None;
                self.announce(QueueEvent::CurrentTrackChanged { id: None });
            }
        }
    }

    pub(super) fn cancel_target_answers(&mut self) {
        if let Some(target) = self.target {
            let seq = self
                .active
                .iter()
                .find(|active| {
                    active.item == target.to && matches!(active.role, Role::Incoming { .. })
                })
                .and_then(|active| match active.role {
                    Role::Incoming { batch } => batch.or(active.load),
                    _ => None,
                })
                .or(target.retry)
                .or(target.repeat);
            if let Some(seq) = seq {
                self.finish_answers(seq, Err(PlayError::NotReady));
            }
        }
    }

    pub(super) fn retry_load(
        &mut self,
        index: usize,
        previous: Seq,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        let id = self.active.get(index).ok_or(PlayError::NoActiveSlot)?.item;
        let source = self
            .tracks
            .source(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let observer = self
            .tracks
            .observer(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let loader = self.loader.as_ref().ok_or(PlayError::NotReady)?;
        let (item, load) = loader.start(id, source, observer)?;
        let seq = self
            .active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .track
            .apply(
                TrackCommand::Load {
                    item,
                    position: Position::ZERO,
                },
                out,
            )?;
        self.active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .load = seq;
        self.tracks.begin_load(id, load);
        if let Some(seq) = seq {
            self.retarget_answers(previous, seq);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use kithara_platform::sync::Arc;
    use kithara_play::{ItemRole, PlayerEvent, SlotId, TrackRef};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        event::QueueEvent,
        queue::state::tests::{make_queue, wait_for_queue_event},
    };

    fn append(queue: &mut Queue<crate::test_pools::TestPools>, source: &str) -> TrackId {
        queue
            .append(source)
            .expect("BUG: open queue must accept a track")
    }

    #[kithara::test(tokio)]
    async fn len_is_empty_reflect_append() {
        let (mut queue, _audio_thread) = make_queue();
        assert!(queue.is_empty());
        let _ = append(&mut queue, "https://example.com/a.mp3");
        let _ = append(&mut queue, "https://example.com/b.mp3");
        assert_eq!(queue.len(), 2);
    }

    #[kithara::test(tokio)]
    async fn append_returns_monotonic_ids_and_emits_track_added() {
        let (mut queue, _audio_thread) = make_queue();
        let mut rx = queue.subscribe();
        let a = append(&mut queue, "https://example.com/a.mp3");
        let b = append(&mut queue, "https://example.com/b.mp3");
        assert_ne!(a, b);
        assert!(a.as_u64() < b.as_u64());

        let mut seen = 0;
        while wait_for_queue_event(
            &mut rx,
            |ev| matches!(ev, QueueEvent::TrackAdded { .. }),
            200,
        )
        .await
        {
            seen += 1;
            if seen == 2 {
                break;
            }
        }
        assert_eq!(seen, 2);
    }

    #[kithara::test(tokio)]
    async fn remove_drops_from_queue_and_emits() {
        let (mut queue, _audio_thread) = make_queue();
        let a = append(&mut queue, "https://example.com/a.mp3");
        let _b = append(&mut queue, "https://example.com/b.mp3");
        let mut rx = queue.subscribe();

        queue
            .remove(a)
            .expect("BUG: just-appended track must be removable");
        assert_eq!(queue.len(), 1);
        let saw_removed = wait_for_queue_event(
            &mut rx,
            |ev| matches!(ev, QueueEvent::TrackRemoved { id } if id == &a),
            300,
        )
        .await;
        assert!(saw_removed);
    }

    #[kithara::test(tokio)]
    async fn clear_empties_queue() {
        let (mut queue, _audio_thread) = make_queue();
        let _a = append(&mut queue, "https://example.com/a.mp3");
        let _b = append(&mut queue, "https://example.com/b.mp3");
        assert_eq!(queue.len(), 2);
        queue.clear().expect("the idle deck takes the clear");
        assert_eq!(queue.len(), 0);
    }

    #[kithara::test(tokio)]
    async fn clear_discards_old_eof_before_reinsert() {
        let (mut queue, _audio_thread) = make_queue();
        let old = queue
            .append("https://example.com/old.mp3")
            .expect("open queue accepts a track");
        queue.navigation.select(old, &[old]);
        queue.player.bus().publish(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Leading(TrackRef::new(
                old,
                SlotId::new(0),
                Arc::from(format!("test://memory/{}", old.as_u64())),
            )),
        });

        queue.clear().expect("the idle deck takes the clear");
        let replacement = queue
            .append("https://example.com/replacement.mp3")
            .expect("open queue accepts a replacement track");
        queue.navigation.select(replacement, &[replacement]);
        queue
            .resident
            .set_rate(1.0)
            .expect("a finite rate is accepted");

        queue
            .tick()
            .expect("tick must accept a freshly reinserted queue");

        assert_eq!(
            queue.current().map(|entry| entry.id),
            Some(replacement),
            "an EOF queued before clear must not end the replacement queue"
        );
    }

    #[kithara::test(tokio)]
    async fn set_tracks_replaces_queue() {
        let (mut queue, _audio_thread) = make_queue();
        let _a = append(&mut queue, "https://example.com/a.mp3");
        queue
            .set_tracks(
                [
                    "https://example.com/1.mp3",
                    "https://example.com/2.mp3",
                    "https://example.com/3.mp3",
                ]
                .map(TrackSource::from)
                .into(),
            )
            .expect("the idle deck takes the clear");
        assert_eq!(queue.len(), 3);
    }

    #[kithara::test(tokio)]
    async fn insert_after_id_places_next() {
        let (mut queue, _audio_thread) = make_queue();
        let a = append(&mut queue, "https://example.com/a.mp3");
        let b = append(&mut queue, "https://example.com/b.mp3");
        let mid = queue
            .insert("https://example.com/mid.mp3", Some(a))
            .expect("BUG: insert relative to existing track");
        let snapshot = queue.tracks();
        let ids: Vec<TrackId> = snapshot.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![a, mid, b]);
    }

    #[kithara::test(tokio)]
    async fn track_source_is_keyed_by_id_across_removal() {
        let (mut queue, _audio_thread) = make_queue();
        let a = append(&mut queue, "https://example.com/a.mp3");
        let b = append(&mut queue, "https://example.com/b.mp3");

        assert_eq!(
            queue
                .track_source(a)
                .and_then(|s| s.uri().map(str::to_string)),
            Some("https://example.com/a.mp3".to_string()),
            "source resolves by identity"
        );

        // Removing an earlier track must not shift which source `b` resolves
        // to, and the removed id must no longer have a source.
        queue.remove(a).expect("BUG: remove existing track");
        assert!(
            queue.track_source(a).is_none(),
            "removed track has no source"
        );
        assert_eq!(
            queue
                .track_source(b)
                .and_then(|s| s.uri().map(str::to_string)),
            Some("https://example.com/b.mp3".to_string()),
            "surviving track still resolves to its own source by id"
        );
    }
}
