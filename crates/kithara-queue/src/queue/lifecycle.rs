use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::SelectionPlayback;

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

    /// Remove every track, as [`QueueControl::clear`](super::QueueControl::clear)
    /// does, while the caller still owns this queue.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed, and
    /// the deck's refusal to clear; the queue keeps its tracks then.
    pub(crate) fn clear(&mut self) -> Result<(), QueueError> {
        self.with_open_result(Self::clear_inner)
    }

    fn clear_inner(&mut self) -> Result<(), QueueError> {
        self.player.remove_all_items()?;
        let ids = self.track_ids();
        self.tracks.records_mut().clear();

        self.pending_select = SelectPhase::Idle;
        let repeat = self.navigation.repeat_mode();
        let order = self.navigation.playback_order();
        self.navigation = NavigationState::new(self.navigation.history_limit());
        self.navigation.set_repeat(repeat);
        self.navigation.set_playback_order(order, &[]);
        self.position = CachedPosition::Unknown;
        self.autoplay_target = None;
        self.player_rx = self.bus.subscribe();
        for id in ids {
            self.announce(QueueEvent::TrackRemoved { id });
        }
        Ok(())
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

        let records = self.tracks.records_mut();
        let index = match placement {
            Placement::Append => {
                records.push(record);
                records.len() - 1
            }
            Placement::At(pos) => {
                records.insert(pos, record);
                pos
            }
        };
        self.announce(QueueEvent::TrackAdded { id, index });
        let ids = self.track_ids();
        self.navigation.reconcile(&ids);
        self.navigation.insert(id);
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
        let pos = match after {
            None => 0,
            Some(after_id) => self
                .tracks
                .records()
                .iter()
                .position(|e| e.id == after_id)
                .map(|i| i + 1)
                .ok_or(QueueError::UnknownTrackId(after_id))?,
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
        let records = self.tracks.records();
        let pos = records
            .iter()
            .position(|e| e.id == id)
            .ok_or(QueueError::UnknownTrackId(id))?;
        let successor_id = was_current
            .then(|| {
                let next = records.get(pos + 1);
                let prev = pos.checked_sub(1).and_then(|p| records.get(p));
                next.or(prev).map(|e| e.id)
            })
            .flatten();
        drop(self.tracks.records_mut().remove(pos));
        if self.player.armed_next() == Some(id) {
            self.player.unarm_next();
        }
        self.announce(QueueEvent::TrackRemoved { id });

        let ids = self.track_ids();
        let order = self.navigation.playback_order();
        self.navigation.reconcile(&ids);

        if was_current {
            let replacement = match order {
                PlaybackOrder::Sequential => {
                    successor_id.filter(|candidate| ids.contains(candidate))
                }
                PlaybackOrder::Shuffle => self.navigation.next(&ids, false, false),
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

    /// Replace every track with `sources`, as
    /// [`QueueControl::set_tracks`](super::QueueControl::set_tracks) does,
    /// while the caller still owns this queue.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed, and
    /// the deck's refusal to clear; the queue keeps its tracks then.
    pub(crate) fn set_tracks(&mut self, sources: Vec<TrackSource<S>>) -> Result<(), QueueError> {
        self.with_open_result(|queue| {
            queue.clear_inner()?;
            for source in sources {
                queue.insert_entry(TrackId::allocate(), source, Placement::Append);
            }
            Ok(())
        })
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
        queue.clear().expect("the idle deck takes the clear");
        assert_eq!(queue.len(), 0);
    }

    #[kithara::test(tokio)]
    async fn clear_discards_old_eof_before_reinsert() {
        let mut queue = make_queue();
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
