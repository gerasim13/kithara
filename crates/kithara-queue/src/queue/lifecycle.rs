use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::SelectionPlayback;
use smallvec::SmallVec;

use super::{
    Queue,
    types::{
        CachedPosition, PendingSelect, Placement, SelectPhase, Transition, extract_track_name,
    },
};
use crate::{
    attempts::LoadClass,
    error::QueueError,
    event::{AdvanceReason, QueueEvent},
    navigation::{NavigationState, PlaybackOrder},
    track::{TrackRecord, TrackSource},
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Append a track, as [`QueueControl::append`](super::QueueControl::append)
    /// does, while the caller still owns this queue.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append<T: Into<TrackSource<S>>>(&mut self, source: T) -> Result<TrackId, QueueError> {
        self.append_with_id(TrackId::allocate(), source)
    }

    /// Append a track with a caller-owned id from [`TrackId::allocate`].
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append_with_id<T: Into<TrackSource<S>>>(
        &mut self,
        id: TrackId,
        source: T,
    ) -> Result<TrackId, QueueError> {
        let source = source.into();
        self.with_open(|queue| queue.insert_entry(id, source, Placement::Append))
            .map_err(QueueError::from)
    }

    pub(crate) fn clear(&mut self) {
        self.command(Self::clear_inner);
    }

    fn clear_inner(&mut self) {
        let ids: Vec<TrackId> = {
            let mut guard = self.lock_tracks_mut();
            let ids = guard.iter().map(|r| r.id).collect();
            guard.clear();
            drop(guard);

            self.pending_select = SelectPhase::Idle;
            let mut navigation = self.lock_navigation_mut();
            let repeat = navigation.repeat_mode();
            let order = navigation.playback_order();
            *navigation = NavigationState::new(navigation.history_limit());
            navigation.set_repeat(repeat);
            navigation.set_playback_order(order, &[]);
            drop(navigation);
            self.write_cached_position(CachedPosition::Unknown);
            self.autoplay_target = None;
            self.player.remove_all_items();
            ids
        };
        self.player_rx = self.bus.subscribe();
        for id in ids {
            self.bus.publish(QueueEvent::TrackRemoved { id });
        }
    }

    /// Insert a track after `after`, or at the head when it is absent, as
    /// [`QueueControl::insert`](super::QueueControl::insert) does, while the
    /// caller still owns this queue.
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `after` does not match any
    /// track.
    pub fn insert<T: Into<TrackSource<S>>>(
        &mut self,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        self.insert_with_id(TrackId::allocate(), source, after)
    }

    /// Inserts a resolved track placement into queue state, takes a successor
    /// it displaces off the deck, and starts loading.
    pub(super) fn insert_entry(
        &mut self,
        id: TrackId,
        source: TrackSource<S>,
        placement: Placement,
    ) -> TrackId {
        let record = TrackRecord::new(id, extract_track_name(&source), source.clone());
        if self.current().is_none() && self.autoplay_target.is_none() {
            self.autoplay_target = Some(id);
            self.override_pending_select(PendingSelect {
                id,
                settings: Transition::None.settings(self.crossfade_settings()),
                playback: if self.config.should_autoplay {
                    SelectionPlayback::Play
                } else {
                    SelectionPlayback::Pause
                },
                reason: AdvanceReason::InitialLoad,
            });
        }

        let index = {
            let mut guard = self.lock_tracks_mut();
            match placement {
                Placement::Append => {
                    guard.push(record);
                    guard.len() - 1
                }
                Placement::At(pos) => {
                    guard.insert(pos, record);
                    pos
                }
            }
        };
        self.bus.publish(QueueEvent::TrackAdded { id, index });
        let ids = self
            .tracks()
            .into_iter()
            .map(|track| track.id)
            .collect::<SmallVec<[_; 16]>>();
        let mut navigation = self.lock_navigation_mut();
        navigation.reconcile(&ids);
        navigation.insert(id);
        drop(navigation);
        self.reconcile_successor();
        self.spawn_apply_after_load(id, source, LoadClass::Prefetch);
        id
    }

    /// Insert a track with a caller-owned id from [`TrackId::allocate`].
    ///
    /// # Errors
    /// Returns [`QueueError::UnknownTrackId`] if `after` does not match
    /// any track.
    pub fn insert_with_id<T: Into<TrackSource<S>>>(
        &mut self,
        id: TrackId,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        let source = source.into();
        self.with_open_result(|queue| queue.insert_with_id_inner(id, source, after))
    }

    fn insert_with_id_inner(
        &mut self,
        id: TrackId,
        source: TrackSource<S>,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        let pos = {
            let guard = self.lock_tracks();
            match after {
                None => 0,
                Some(after_id) => guard
                    .iter()
                    .position(|e| e.id == after_id)
                    .map(|i| i + 1)
                    .ok_or(QueueError::UnknownTrackId(after_id))?,
            }
        };
        Ok(self.insert_entry(id, source, Placement::At(pos)))
    }

    pub(crate) fn remove(&mut self, id: TrackId) -> Result<(), QueueError> {
        self.with_open_result(|queue| queue.remove_inner(id))
    }

    fn remove_inner(&mut self, id: TrackId) -> Result<(), QueueError> {
        let was_current = self.current().map(|e| e.id) == Some(id);
        let playback = if self.player.is_playing() {
            SelectionPlayback::Play
        } else {
            SelectionPlayback::Pause
        };
        let successor_id = if was_current {
            let guard = self.lock_tracks();
            let pos = guard.iter().position(|e| e.id == id);
            let result = pos.and_then(|p| {
                let next = guard.get(p + 1);
                let prev = if p > 0 { guard.get(p - 1) } else { None };
                next.or(prev).map(|e| e.id)
            });
            drop(guard);
            result
        } else {
            None
        };

        let removed = {
            let mut guard = self.lock_tracks_mut();
            let pos = guard
                .iter()
                .position(|e| e.id == id)
                .ok_or(QueueError::UnknownTrackId(id))?;
            guard.remove(pos)
        };
        drop(removed);
        if self.player.armed_next() == Some(id) {
            self.player.unarm_next();
        }
        self.bus.publish(QueueEvent::TrackRemoved { id });

        let entries = self.tracks();
        let ids = entries
            .iter()
            .map(|entry| entry.id)
            .collect::<SmallVec<[_; 16]>>();
        let order = self.lock_navigation().playback_order();
        self.lock_navigation_mut().reconcile(&ids);

        if was_current {
            let replacement = match order {
                PlaybackOrder::Sequential => {
                    successor_id.filter(|candidate| ids.contains(candidate))
                }
                PlaybackOrder::Shuffle => self.lock_navigation_mut().next(&ids, false, false),
            };
            if let Some(next) = replacement {
                self.select_with(
                    next,
                    Transition::None,
                    AdvanceReason::RemovedCurrent,
                    playback,
                )?;
                self.commit_navigation_to(next);
            } else {
                self.player.pause();
            }
        }
        Ok(())
    }

    pub(crate) fn set_tracks(&mut self, sources: Vec<TrackSource<S>>) {
        self.command(|queue| {
            queue.clear_inner();
            for source in sources {
                queue.insert_entry(TrackId::allocate(), source, Placement::Append);
            }
        });
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
        let mut queue = make_queue();
        assert!(queue.is_empty());
        let _ = append(&mut queue, "https://example.com/a.mp3");
        let _ = append(&mut queue, "https://example.com/b.mp3");
        assert_eq!(queue.len(), 2);
    }

    #[kithara::test(tokio)]
    async fn append_returns_monotonic_ids_and_emits_track_added() {
        let mut queue = make_queue();
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
        let mut queue = make_queue();
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
        let mut queue = make_queue();
        let _a = append(&mut queue, "https://example.com/a.mp3");
        let _b = append(&mut queue, "https://example.com/b.mp3");
        assert_eq!(queue.len(), 2);
        queue.clear();
        assert_eq!(queue.len(), 0);
    }

    #[kithara::test(tokio)]
    async fn clear_discards_old_eof_before_reinsert() {
        let mut queue = make_queue();
        let old = queue
            .append("https://example.com/old.mp3")
            .expect("open queue accepts a track");
        queue.lock_navigation_mut().select(old, &[old]);
        queue.player.bus().publish(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Leading(TrackRef::new(
                old,
                SlotId::new(0),
                Arc::from(format!("test://memory/{}", old.as_u64())),
            )),
        });

        queue.clear();
        let replacement = queue
            .append("https://example.com/replacement.mp3")
            .expect("open queue accepts a replacement track");
        queue
            .lock_navigation_mut()
            .select(replacement, &[replacement]);
        queue.player.set_rate(1.0);

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
        let mut queue = make_queue();
        let _a = append(&mut queue, "https://example.com/a.mp3");
        queue.set_tracks(
            [
                "https://example.com/1.mp3",
                "https://example.com/2.mp3",
                "https://example.com/3.mp3",
            ]
            .map(TrackSource::from)
            .into(),
        );
        assert_eq!(queue.len(), 3);
    }

    #[kithara::test(tokio)]
    async fn insert_after_id_places_next() {
        let mut queue = make_queue();
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
        let mut queue = make_queue();
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
