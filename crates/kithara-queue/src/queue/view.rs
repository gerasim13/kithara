use arc_swap::ArcSwap;
use kithara_audio::AudioObserver;
use kithara_bufpool::HasPool;
use kithara_events::{EventReceiver, EventSet, TrackId};
use kithara_platform::sync::Arc;
use kithara_play::{
    EngineLoadSnapshot, Player, PlayerStatus, SlotSnapshot, TrackFactory, TrackSettings,
    TrackSnapshot, TrackStatus as PlayingStatus,
};

use super::{PlaybackView, Queue, QueueControl};
use crate::{
    ActionAtItemEnd, NavigationState, PlaybackOrder, QueueSettings, RepeatMode, TrackEntry,
    TrackSource, TrackStatus,
    track::{TrackRecord, TrackRow, Tracks},
};

/// The queue's published rows, sounding track and navigation settings.
#[derive_where::derive_where(Clone)]
pub struct QueueSnapshot<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub current: Option<TrackId>,
    pub track: Option<TrackSnapshot>,
    pub settings: QueueSettings,
    rows: Arc<[TrackRow<S>]>,
    revision: u64,
    order: PlaybackOrder,
    repeat: RepeatMode,
    action: ActionAtItemEnd,
    initial: TrackSettings,
    slot: Option<SlotSnapshot>,
    sample_rate: u32,
}

/// Handles only read this snapshot; the queue is its sole publisher.
#[derive_where::derive_where(Clone)]
pub(crate) struct QueueView<S>(Arc<ArcSwap<QueueSnapshot<S>>>)
where
    S: HasPool<u8> + Send + Sync + 'static;

impl<S> QueueView<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(crate) fn new(
        tracks: &Tracks<S>,
        navigation: &NavigationState,
        settings: QueueSettings,
        initial: TrackSettings,
        action: ActionAtItemEnd,
    ) -> Self {
        Self(Arc::new(ArcSwap::from_pointee(QueueSnapshot {
            current: None,
            track: None,
            settings,
            rows: tracks.rows(),
            revision: tracks.revision(),
            order: navigation.playback_order(),
            repeat: navigation.repeat_mode(),
            action,
            initial,
            slot: None,
            sample_rate: 0,
        })))
    }

    pub(super) fn read(&self) -> Arc<QueueSnapshot<S>> {
        self.0.load_full()
    }

    pub(super) fn publish(&self, snapshot: QueueSnapshot<S>) {
        self.0.store(Arc::new(snapshot));
    }

    pub(super) fn attach_observer(&self, id: TrackId, observer: Box<dyn AudioObserver>) {
        if let Some(row) = self.0.load().rows.iter().find(|row| row.entry.id == id) {
            row.observer.attach(observer);
        }
    }
}

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) fn queue_snapshot(&self) -> QueueSnapshot<S> {
        let track = self
            .current_track()
            .map(|track| track.snapshot().as_ref().clone());
        let slot = track
            .as_ref()
            .and_then(|track| self.deck.slots.get(usize::from(track.slot.get())).copied());
        let published = self.view.read();
        let rows = if published.revision == self.tracks.revision() {
            Arc::clone(&published.rows)
        } else {
            self.tracks.rows()
        };
        QueueSnapshot {
            current: self.current,
            track,
            settings: self.config.settings,
            rows,
            revision: self.tracks.revision(),
            order: self.navigation.playback_order(),
            repeat: self.navigation.repeat_mode(),
            action: self.config.action_at_item_end,
            initial: self.config.track,
            slot,
            sample_rate: self.deck.sample_rate,
        }
    }

    #[must_use]
    pub fn current(&self) -> Option<TrackEntry> {
        self.track(self.current?)
    }

    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        let id = self.current?;
        self.tracks
            .records()
            .iter()
            .position(|record| record.id == id)
    }

    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<TrackEntry> {
        self.tracks
            .records()
            .iter()
            .find(|record| record.id == id)
            .map(TrackRecord::entry)
    }

    #[must_use]
    pub fn tracks(&self) -> Vec<TrackEntry> {
        self.tracks
            .records()
            .iter()
            .map(TrackRecord::entry)
            .collect()
    }

    #[must_use]
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }
}

impl<S> QueueControl<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    #[must_use]
    pub fn current(&self) -> Option<TrackEntry> {
        let snapshot = self.view.read();
        let id = snapshot.current?;
        snapshot
            .rows
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.entry.clone())
    }

    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        let snapshot = self.view.read();
        let id = snapshot.current?;
        snapshot.rows.iter().position(|row| row.entry.id == id)
    }

    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<TrackEntry> {
        self.view
            .read()
            .rows
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.entry.clone())
    }

    #[must_use]
    pub fn tracks(&self) -> Vec<TrackEntry> {
        self.view
            .read()
            .rows
            .iter()
            .map(|row| row.entry.clone())
            .collect()
    }

    #[must_use]
    pub fn track_source(&self, id: TrackId) -> Option<TrackSource<S>> {
        self.view
            .read()
            .rows
            .iter()
            .find(|row| row.entry.id == id)
            .map(|row| row.source.clone())
    }

    pub fn attach_observer<O: AudioObserver>(&self, id: TrackId, observer: O) {
        self.view.attach_observer(id, Box::new(observer));
    }

    #[must_use]
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    #[must_use]
    pub fn bus(&self) -> &kithara_events::EventBus {
        &self.bus
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.view.read().rows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.view.read().rows.is_empty()
    }

    #[must_use]
    pub fn playback_order(&self) -> PlaybackOrder {
        self.view.read().order
    }

    #[must_use]
    pub fn repeat_mode(&self) -> RepeatMode {
        self.view.read().repeat
    }

    #[must_use]
    pub fn action_at_item_end(&self) -> ActionAtItemEnd {
        self.view.read().action
    }

    #[must_use]
    pub fn crossfade_settings(&self) -> kithara_play::CrossfadeSettings {
        self.view.read().settings.crossfade()
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.view.read().sample_rate
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.view
            .read()
            .track
            .as_ref()
            .is_some_and(|track| matches!(track.status, PlayingStatus::Playing { .. }))
    }

    #[must_use]
    pub fn rate(&self) -> f32 {
        let snapshot = self.view.read();
        snapshot
            .track
            .as_ref()
            .filter(|track| matches!(track.status, PlayingStatus::Playing { .. }))
            .map_or(0.0, |track| track.speed)
    }

    #[must_use]
    pub fn default_rate(&self) -> f32 {
        let snapshot = self.view.read();
        snapshot
            .track
            .as_ref()
            .map_or(snapshot.initial.speed(), |track| track.speed)
    }

    #[must_use]
    pub fn position_seconds(&self) -> Option<f64> {
        let snapshot = self.view.read();
        snapshot
            .track
            .as_ref()
            .map(|track| track.position.as_secs_f64())
    }

    #[must_use]
    pub fn duration_seconds(&self) -> Option<f64> {
        let snapshot = self.view.read();
        snapshot
            .track
            .as_ref()
            .and_then(|track| track.duration.map(|duration| duration.as_secs_f64()))
    }

    #[must_use]
    pub fn status(&self) -> PlayerStatus {
        let snapshot = self.view.read();
        if snapshot.current.is_some_and(|id| {
            snapshot
                .rows
                .iter()
                .any(|row| row.entry.id == id && matches!(row.entry.status, TrackStatus::Failed(_)))
        }) {
            PlayerStatus::Failed
        } else if snapshot.track.as_ref().is_some_and(|track| {
            !matches!(
                track.status,
                PlayingStatus::Idle | PlayingStatus::Loading | PlayingStatus::Released
            )
        }) {
            PlayerStatus::ReadyToPlay
        } else {
            PlayerStatus::Unknown
        }
    }

    #[must_use]
    pub fn current_abr_handle(&self) -> Option<kithara_abr::AbrHandle> {
        self.view
            .read()
            .track
            .as_ref()
            .and_then(|track| track.abr.clone())
    }

    #[must_use]
    pub fn current_variant(&self) -> Option<kithara_abr::VariantInfo> {
        self.current_abr_handle()?.current_variant()
    }

    #[must_use]
    pub fn playback_view(&self) -> PlaybackView {
        let snapshot = self.view.read();
        let Some(track) = &snapshot.track else {
            return PlaybackView::default();
        };
        PlaybackView {
            buffered: snapshot.slot.map(|slot| slot.frontier.max(slot.cached)),
            duration: track.duration.map(|duration| duration.as_secs_f64()),
            position: Some(track.position.as_secs_f64()),
            playing: matches!(track.status, PlayingStatus::Playing { .. }),
        }
    }

    #[must_use]
    pub fn engine_load(&self) -> EngineLoadSnapshot {
        todo!(
            "kithara-host DeckPass worker load observation for the published queue snapshot; DeckSnapshot only carries RT counters (contract §8.1; skeleton queue view)"
        )
    }

    #[must_use]
    pub fn volume(&self) -> f32 {
        todo!(
            "kithara-host DeckPass published applied deck mix volume (contract §8.4; skeleton queue view)"
        )
    }

    #[must_use]
    pub fn is_muted(&self) -> bool {
        todo!(
            "kithara-host DeckPass published applied deck mute (contract §8.4; skeleton queue view)"
        )
    }

    #[must_use]
    pub fn eq_band_count(&self) -> usize {
        todo!(
            "kithara-host DeckPass published applied deck EQ layout (contract §8.4; skeleton queue view)"
        )
    }

    #[must_use]
    pub fn eq_gain(&self, band: usize) -> Option<f32> {
        todo!(
            "kithara-host DeckPass published applied deck EQ band {band} (contract §8.4; skeleton queue view)"
        )
    }
}
