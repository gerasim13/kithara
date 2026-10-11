use kithara_bufpool::HasPool;
use kithara_command::{Seq, When};
use kithara_events::TrackId;
use kithara_play::{
    Outbox, OutputSnapshot, PlayError, Player, PlayerConfig, Position, Slot, Track, TrackCommand,
    TrackFactory, TrackStatus as PlayingStatus,
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

    pub(super) fn autoplay(
        &mut self,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        if self.config.should_autoplay
            && self.current.is_none()
            && self.target.is_none()
            && self.navigation.current().is_none()
        {
            self.next_target(
                Transition::None,
                AdvanceReason::InitialLoad,
                false,
                output,
                out,
            )?;
        }
        Ok(())
    }

    pub(super) fn remove_entry(
        &mut self,
        id: TrackId,
        output: Option<&OutputSnapshot>,
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
                output,
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
        self.target = None;
        self.held_position = None;
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
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        if let Some(index) = self.active.position(|active| {
            active.item == id && matches!(active.role, Role::Incoming { .. } | Role::Preloaded)
        }) {
            let active = self.active.get_mut(index).ok_or(QueueError::NotReady(id))?;
            active.role = role;
            return Ok(active.load.map(super::slots::LoadState::seq));
        }
        let Some(slot) = self.active.free_slot() else {
            return self.evict_for(id, role, output, out);
        };
        let active = self.prepare_track(id, slot, role, output, out)?;
        let seq = active.load.map(super::slots::LoadState::seq);
        self.active.push(active);
        Ok(seq)
    }

    fn prepare_track(
        &mut self,
        id: TrackId,
        slot: Slot,
        role: Role,
        output: Option<&OutputSnapshot>,
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
        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| PlayError::InvalidConfiguration {
                reason: "a hosted queue requires resource preparation and an asset store".into(),
            })?;
        let output = output.ok_or(PlayError::NotReady)?;
        let (item, load) = loader.start(id, source, observer, output)?;
        let position = self.held_position.unwrap_or(Position::ZERO);
        let seq = track.apply(TrackCommand::Load { item, position }, out)?;
        self.held_position = None;
        self.tracks.begin_load(id, load);
        Ok(Active {
            item: id,
            slot,
            track,
            role,
            load: seq.map(super::slots::LoadState::Opening),
        })
    }

    fn evict_for(
        &mut self,
        id: TrackId,
        role: Role,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        if let Some(index) = self.active.replacement_index() {
            self.release_track(index, out)?;
            self.reap_released();
        }
        let victim = self
            .active
            .quietest(&self.deck.mixer, |_| true)
            .ok_or_else(|| PlayError::InvalidConfiguration {
                reason: "a mixer must have at least one slot".into(),
            })?;
        let slot = self
            .active
            .get(victim)
            .ok_or(QueueError::NotReady(id))?
            .slot;
        let mut replacement = self.prepare_track(id, slot, role, output, out)?;
        replacement
            .track
            .apply(TrackCommand::Evict { at: When::Next }, out)?;
        let seq = replacement.load.map(super::slots::LoadState::seq);
        self.active.stage(replacement);
        Ok(seq)
    }

    pub(super) fn release_track(
        &mut self,
        index: usize,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
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

    pub(super) fn retry_load(
        &mut self,
        index: usize,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        let active = self.active.get(index).ok_or(PlayError::NoActiveSlot)?;
        let id = active.item;
        let position = active.track.snapshot().as_ref().position;
        let source = self
            .tracks
            .source(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let observer = self
            .tracks
            .observer(id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let loader = self.loader.as_ref().ok_or(PlayError::NotReady)?;
        let output = output.ok_or(PlayError::NotReady)?;
        let (item, load) = loader.start(id, source, observer, output)?;
        let seq = self
            .active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .track
            .apply(TrackCommand::Load { item, position }, out)?;
        self.active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .load = seq.map(super::slots::LoadState::Opening);
        self.tracks.begin_load(id, load);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_host::{Host, HostConfig, HostOwned};
    use kithara_platform::tokio::task::spawn_blocking;
    use kithara_play::{
        DeckEvent, HostedDeck, PlayWorker, PlayWorkerConfig, ResourcePrep, Slot, TrackReceipt,
    };
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        QueueConfig, QueueControl,
        event::QueueEvent,
        queue::{
            QueueCommand,
            state::tests::{apply, make_queue, wait_for_queue_event, with_outbox},
        },
        test_pools::{TestPools, pools},
    };

    async fn hosted_queue() -> (HostOwned<Queue<TestPools>>, Host<TestPools>) {
        spawn_blocking(|| {
            let prep = ResourcePrep::builder()
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .build();
            let config = QueueConfig::builder()
                .prep(prep)
                .store(
                    AssetStore::builder(pools())
                        .backend(StorageBackend::Memory)
                        .build(),
                )
                .build();
            let mut host =
                Host::new(HostConfig::offline(pools()).build()).expect("fixture offline Host");
            let queue = host
                .insert(Queue::new(config))
                .expect("insert fixture queue");
            (queue, host)
        })
        .await
        .expect("create queue on the blocking test worker")
    }

    async fn command<R: Send + 'static>(
        queue: &QueueControl<TestPools>,
        run: impl FnOnce(QueueControl<TestPools>) -> R + Send + 'static,
    ) -> R {
        let control = queue.clone();
        spawn_blocking(move || run(control))
            .await
            .expect("queue command worker must not panic")
    }

    async fn append(queue: &QueueControl<TestPools>, source: &str) -> TrackId {
        let source = source.to_owned();
        command(queue, move |control| {
            control
                .append(source)
                .expect("BUG: open queue must accept a track")
        })
        .await
    }

    #[kithara::test(tokio)]
    async fn len_is_empty_reflect_append() {
        let (queue, _host) = hosted_queue().await;
        assert!(queue.is_empty());
        let _ = append(&queue, "https://example.com/a.mp3").await;
        let _ = append(&queue, "https://example.com/b.mp3").await;
        assert_eq!(queue.len(), 2);
    }

    #[kithara::test(tokio)]
    async fn append_returns_monotonic_ids_and_emits_track_added() {
        let (queue, _host) = hosted_queue().await;
        let mut rx = queue.subscribe();
        let a = append(&queue, "https://example.com/a.mp3").await;
        let b = append(&queue, "https://example.com/b.mp3").await;
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
        let (queue, _host) = hosted_queue().await;
        let a = append(&queue, "https://example.com/a.mp3").await;
        let _b = append(&queue, "https://example.com/b.mp3").await;
        let mut rx = queue.subscribe();

        command(&queue, move |control| control.remove(a))
            .await
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
        let (queue, _host) = hosted_queue().await;
        let _a = append(&queue, "https://example.com/a.mp3").await;
        let _b = append(&queue, "https://example.com/b.mp3").await;
        assert_eq!(queue.len(), 2);
        command(&queue, |control| control.clear())
            .await
            .expect("the idle deck takes the clear");
        assert_eq!(queue.len(), 0);
    }

    #[kithara::test(tokio)]
    async fn clear_discards_old_eof_before_reinsert() {
        let (mut queue, mut rig) = make_queue();
        let old = TrackId::allocate();
        apply(
            &mut queue,
            &mut rig,
            QueueCommand::Append {
                id: old,
                source: "https://example.com/old.mp3".into(),
            },
        )
        .expect("open queue accepts a track");
        queue.navigation.select(old, &[old]);
        let ended = DeckEvent::Ended {
            slot: Slot::new(0),
            at: kithara_signal::SessionFrame::new(0),
        };

        apply(&mut queue, &mut rig, QueueCommand::RemoveAll)
            .expect("the idle deck takes the clear");
        let replacement = TrackId::allocate();
        apply(
            &mut queue,
            &mut rig,
            QueueCommand::Append {
                id: replacement,
                source: "https://example.com/replacement.mp3".into(),
            },
        )
        .expect("open queue accepts a replacement track");
        queue.navigation.select(replacement, &[replacement]);
        with_outbox(&mut queue, &mut rig, |queue, pass, out| {
            HostedDeck::settle(queue, TrackReceipt::Event(ended), pass, out);
            HostedDeck::tick(queue, pass, out);
        });

        assert_eq!(
            queue.control().current().map(|entry| entry.id),
            Some(replacement),
            "an EOF queued before clear must not end the replacement queue"
        );
    }

    #[kithara::test(tokio)]
    async fn set_tracks_replaces_queue() {
        let (queue, _host) = hosted_queue().await;
        let _a = append(&queue, "https://example.com/a.mp3").await;
        command(&queue, |control| {
            control.set_tracks(
                [
                    "https://example.com/1.mp3",
                    "https://example.com/2.mp3",
                    "https://example.com/3.mp3",
                ]
                .map(TrackSource::from),
            )
        })
        .await
        .expect("the idle deck takes the clear");
        assert_eq!(queue.len(), 3);
    }

    #[kithara::test(tokio)]
    async fn insert_after_id_places_next() {
        let (queue, _host) = hosted_queue().await;
        let a = append(&queue, "https://example.com/a.mp3").await;
        let b = append(&queue, "https://example.com/b.mp3").await;
        let mid = command(&queue, move |control| {
            control.insert("https://example.com/mid.mp3", Some(a))
        })
        .await
        .expect("BUG: insert relative to existing track");
        let snapshot = queue.tracks();
        let ids: Vec<TrackId> = snapshot.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![a, mid, b]);
    }

    #[kithara::test(tokio)]
    async fn track_source_is_keyed_by_id_across_removal() {
        let (queue, _host) = hosted_queue().await;
        let a = append(&queue, "https://example.com/a.mp3").await;
        let b = append(&queue, "https://example.com/b.mp3").await;

        assert_eq!(
            queue
                .track_source(a)
                .and_then(|s| s.uri().map(str::to_string)),
            Some("https://example.com/a.mp3".to_string()),
            "source resolves by identity"
        );

        // Removing an earlier track must not shift which source `b` resolves
        // to, and the removed id must no longer have a source.
        command(&queue, move |control| control.remove(a))
            .await
            .expect("BUG: remove existing track");
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

    #[kithara::test(tokio)]
    async fn begin_dedupes_live_attempt() {
        let (mut queue, mut rig) = make_queue();
        queue.insert_entry(TrackId(1), "/kithara/missing.wav".into(), Placement::Append);
        let first = with_outbox(&mut queue, &mut rig, |queue, pass, out| {
            queue.load_track(TrackId(1), Role::Preloaded, Some(pass.output), out)
        })
        .expect("first load")
        .expect("new load has a sequence");
        let second = with_outbox(&mut queue, &mut rig, |queue, pass, out| {
            queue.load_track(TrackId(1), Role::Preloaded, Some(pass.output), out)
        })
        .expect("duplicate load reuses the open");
        assert_eq!(second, Some(first));
        assert_eq!(queue.active.iter().count(), 1);
        assert_eq!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .expect("one open")
                .seq(),
            first
        );
        assert!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .is_none(),
            "no duplicate open"
        );
    }

    #[kithara::test(tokio)]
    async fn selection_reflects_only_the_requested_live_attempt() {
        let (mut queue, mut rig) = make_queue();
        for id in [TrackId(1), TrackId(2)] {
            queue.insert_entry(id, "/kithara/missing.wav".into(), Placement::Append);
        }
        assert!(!queue.target.is_some_and(|target| target.to == TrackId(1)));
        assert!(
            with_outbox(&mut queue, &mut rig, |queue, pass, out| {
                queue.load_track(TrackId(1), Role::Preloaded, Some(pass.output), out)
            })
            .expect("prefetch")
            .is_some()
        );
        assert!(
            apply(
                &mut queue,
                &mut rig,
                QueueCommand::Select {
                    id: TrackId(2),
                    transition: Transition::None,
                }
            )
            .expect("selected load")
            .is_some()
        );
        assert!(!queue.target.is_some_and(|target| target.to == TrackId(1)));
        assert!(queue.target.is_some_and(|target| target.to == TrackId(2)));
        assert!(
            queue
                .active
                .iter()
                .any(|active| active.item == TrackId(1) && active.role == Role::Preloaded)
        );
        assert!(queue.active.iter().any(
            |active| active.item == TrackId(2) && active.role == Role::Incoming { batch: None }
        ));
    }

    #[kithara::test(tokio)]
    async fn promote_replaces_waiting_and_cancels_it() {
        let (mut queue, mut rig) = make_queue();
        queue.insert_entry(TrackId(1), "/kithara/missing.wav".into(), Placement::Append);
        let queued = with_outbox(&mut queue, &mut rig, |queue, pass, out| {
            queue.load_track(TrackId(1), Role::Preloaded, Some(pass.output), out)
        })
        .expect("queued preload")
        .expect("open sequence");
        let token = queue.tracks.records()[0]
            .load
            .as_ref()
            .expect("queued load")
            .token()
            .clone();
        let parked = queue.tracks.records()[0]
            .load
            .as_ref()
            .expect("queued load")
            .watch()
            .clone();
        let promoted = apply(
            &mut queue,
            &mut rig,
            QueueCommand::Select {
                id: TrackId(1),
                transition: Transition::None,
            },
        )
        .expect("select promotes the queued preload");
        assert!(token.is_cancelled(), "parked attempt must abort");
        assert!(
            parked.is_cancelled(),
            "the replaced attempt loses its claim"
        );
        let replacement = queue.tracks.records()[0]
            .load
            .as_ref()
            .expect("promoted load");
        assert!(
            !replacement.watch().is_cancelled(),
            "the promoted attempt owns its claim"
        );
        assert!(!replacement.token().is_cancelled());
        assert_ne!(promoted, Some(queued));
        assert!(queue.incoming_index(TrackId(1)).is_some());
        assert!(
            !queue
                .active
                .iter()
                .any(|active| active.role == Role::Preloaded)
        );
        assert_eq!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .expect("superseded open")
                .seq(),
            queued
        );
        assert_eq!(
            Some(
                rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                    .expect("promoted open")
                    .seq()
            ),
            promoted
        );
        assert!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .is_none()
        );
    }

    #[kithara::test(tokio)]
    async fn promote_keeps_attempt_holding_permit() {
        let (mut queue, mut rig) = make_queue();
        queue.insert_entry(TrackId(1), "/kithara/missing.wav".into(), Placement::Append);
        let first = with_outbox(&mut queue, &mut rig, |queue, pass, out| {
            queue.load_track(TrackId(1), Role::Preloaded, Some(pass.output), out)
        })
        .expect("preloaded open")
        .expect("open sequence");
        rig.opens.drain();
        let token = queue.tracks.records()[0]
            .load
            .as_ref()
            .expect("admitted load")
            .token()
            .clone();
        let promoted = apply(
            &mut queue,
            &mut rig,
            QueueCommand::Select {
                id: TrackId(1),
                transition: Transition::None,
            },
        )
        .expect("promote admitted open");
        assert_eq!(
            promoted,
            Some(first),
            "an attempt past the permit gate keeps its download"
        );
        assert!(!token.is_cancelled());
        assert_eq!(queue.active.iter().count(), 1);
        assert_eq!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .expect("one open")
                .seq(),
            first
        );
        assert!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .is_none()
        );
    }

    #[kithara::test]
    fn promote_vacant_is_noop() {
        let (mut queue, mut rig) = make_queue();
        queue.insert_entry(TrackId(1), "/kithara/missing.wav".into(), Placement::Append);
        assert!(
            with_outbox(&mut queue, &mut rig, |queue, _, out| {
                queue.transition_loaded(out)
            })
            .expect("no transition to promote")
            .is_none()
        );
        assert!(queue.tracks.records()[0].load.is_none());
        assert_eq!(queue.active.iter().count(), 0);
        assert!(
            rig.open(Err(kithara_play::LoadRefusal::Cancelled))
                .is_none()
        );
    }

    #[kithara::test]
    fn config_failure_without_runtime_updates_tracks_synchronously() {
        use kithara_events::Envelope;

        let (mut queue, mut rig) = make_queue();
        let source: TrackSource<TestPools> = "not a url".into();
        for id in [TrackId(42), TrackId(43)] {
            queue.insert_entry(id, source.clone(), Placement::Append);
        }
        queue.publish();
        let output = kithara_play::mock::output(None).get();
        let Err(expected) =
            queue
                .loader
                .as_ref()
                .expect("loader")
                .build_config(TrackId(42), source, &output)
        else {
            panic!("fixture source must be rejected");
        };
        assert!(matches!(expected, QueueError::InvalidUrl(_)));
        let reason = expected.to_string();
        let mut events = queue.subscribe::<QueueEvent>();
        for (index, id) in [TrackId(42), TrackId(43)].into_iter().enumerate() {
            assert!(matches!(
                with_outbox(&mut queue, &mut rig, |queue, pass, out| {
                    queue.request_transition(
                        id,
                        Transition::None,
                        AdvanceReason::UserSelect,
                        false,
                        Some(pass.output),
                        out,
                    )
                }),
                Err(QueueError::InvalidUrl(_))
            ));
            assert_eq!(
                queue.tracks.records()[index].status,
                crate::TrackStatus::Failed(reason.clone())
            );
            queue.publish();
            assert!(matches!(events.try_recv(), Ok(Envelope {
                event: QueueEvent::TrackStatusChanged { id: changed, status }, ..
            }) if changed == id && status == crate::TrackStatus::Failed(reason.clone())));
        }
        assert!(
            events.try_recv().is_err(),
            "config failure must not emit Loading"
        );
    }
}
