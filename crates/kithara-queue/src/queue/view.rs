use arc_swap::ArcSwap;
use kithara_audio::AudioObserver;
use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;

use super::types::CachedPosition;
use crate::{
    navigation::{NavigationState, PlaybackOrder, RepeatMode},
    track::{TrackEntry, TrackRow, TrackSource, Tracks},
};

/// What the queue last published: its rows, the navigation cursor and modes,
/// and the cached position.
struct QueueSnapshot<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    tracks: Arc<[TrackRow<S>]>,
    /// The [`Tracks::revision`] `tracks` was built at.
    revision: u64,
    current: Option<TrackId>,
    playback_order: PlaybackOrder,
    repeat_mode: RepeatMode,
    position: CachedPosition,
}

/// The queue's state as its handles read it. Only the queue publishes it,
/// before it answers a command and after it drains or ticks, so a handle
/// that waited for an answer reads its own write. The queue's events follow
/// the view that shows them, so a handle that heard a change reads it.
#[derive_where::derive_where(Clone; S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static)]
pub(crate) struct QueueView<S>(Arc<ArcSwap<QueueSnapshot<S>>>)
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static;

impl<S> QueueView<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// A view of `tracks` and `navigation` before the queue knows a position.
    pub(crate) fn new(tracks: &Tracks<S>, navigation: &NavigationState) -> Self {
        Self(Arc::new(ArcSwap::from_pointee(snapshot(
            tracks.rows(),
            tracks,
            navigation,
            CachedPosition::Unknown,
        ))))
    }

    /// Store what the queue holds now. Rows are rebuilt only when the tracks
    /// moved past the revision the published ones were built at.
    pub(super) fn publish(
        &self,
        tracks: &Tracks<S>,
        navigation: &NavigationState,
        position: CachedPosition,
    ) {
        let published = self.0.load();
        let rows = if published.revision == tracks.revision() {
            Arc::clone(&published.tracks)
        } else {
            tracks.rows()
        };
        drop(published);
        self.0
            .store(Arc::new(snapshot(rows, tracks, navigation, position)));
    }

    /// Attach decoded-audio observation to `id`'s decoder; see
    /// [`Tracks::attach_observer`].
    pub(super) fn attach_observer(&self, id: TrackId, observer: Box<dyn AudioObserver>) {
        let slot = self
            .0
            .load()
            .tracks
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.observer.clone());
        if let Some(slot) = slot {
            slot.attach(observer);
        }
    }

    /// Whether the user's selection wants `id`'s live load attempt.
    pub(crate) fn attempt_selected(&self, id: TrackId) -> bool {
        self.0
            .load()
            .tracks
            .iter()
            .any(|row| row.entry.id == id && row.selected)
    }

    pub(super) fn current(&self) -> Option<TrackEntry> {
        let id = self.0.load().current?;
        self.track(id)
    }

    pub(super) fn index_of(&self, id: TrackId) -> Option<usize> {
        self.0
            .load()
            .tracks
            .iter()
            .position(|row| row.entry.id == id)
    }

    delegate::delegate! {
        to self.0.load().tracks {
            pub(super) fn is_empty(&self) -> bool;
            pub(super) fn len(&self) -> usize;
        }
        to self.0 {
            #[expr($.playback_order)]
            #[call(load)]
            pub(crate) fn playback_order(&self) -> PlaybackOrder;
            #[expr($.repeat_mode)]
            #[call(load)]
            pub(super) fn repeat_mode(&self) -> RepeatMode;
        }
    }

    pub(super) fn position_seconds(&self) -> Option<f64> {
        self.0.load().position.into()
    }

    pub(super) fn track(&self, id: TrackId) -> Option<TrackEntry> {
        self.0
            .load()
            .tracks
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.entry.clone())
    }

    pub(super) fn track_source(&self, id: TrackId) -> Option<TrackSource<S>> {
        self.0
            .load()
            .tracks
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.source.clone())
    }

    pub(super) fn tracks(&self) -> Vec<TrackEntry> {
        self.0
            .load()
            .tracks
            .iter()
            .map(|row| row.entry.clone())
            .collect()
    }
}

fn snapshot<S>(
    rows: Arc<[TrackRow<S>]>,
    tracks: &Tracks<S>,
    navigation: &NavigationState,
    position: CachedPosition,
) -> QueueSnapshot<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    QueueSnapshot {
        position,
        tracks: rows,
        revision: tracks.revision(),
        current: navigation.current(),
        playback_order: navigation.playback_order(),
        repeat_mode: navigation.repeat_mode(),
    }
}
