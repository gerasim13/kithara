use kithara_audio::AudioObserver;
use kithara_bufpool::HasPool;
use kithara_events::{EventReceiver, EventSet, TrackId};

use super::{Queue, QueueControl, QueueRuntime};
use crate::{
    event::{QueueEvent, QueueRepeatMode},
    navigation::{ActionAtItemEnd, PlaybackOrder, RepeatMode},
    track::{TrackEntry, TrackRecord, TrackSource},
};

impl<S> QueueRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    #[must_use]
    pub fn action_at_item_end(&self) -> ActionAtItemEnd {
        self.config.action_at_item_end()
    }

    /// Subscribe to the unified event stream:
    /// [`QueueEvent`](crate::event::QueueEvent) + underlying player /
    /// audio / hls / file events.
    #[must_use]
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    delegate::delegate! {
        to self.player {
            /// ABR handle of the currently playing adaptive item, if any.
            ///
            /// Returned handle drives runtime variant/bandwidth control — FFI and
            /// GUI use it for `set_abr_mode` / `set_preferred_peak_bitrate`.
            #[must_use]
            pub fn current_abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            /// Rate the player's master bus runs at, and therefore the frame axis used
            /// by decoded-audio observers attached to this queue.
            #[must_use]
            pub fn sample_rate(&self) -> u32;
        }
        to self {
            /// Live variant metadata of the currently playing adaptive item.
            /// Pulled from the player's stashed ABR handle on every call so a
            /// renderer can poll for the up-to-date label after every frame
            /// without depending on event delivery.
            #[must_use]
            #[expr($?.current_variant())]
            #[call(current_abr_handle)]
            pub fn current_variant(&self) -> Option<kithara_abr::VariantInfo>;
        }
    }
}

/// The queue reads its own state: its logic calls these mid-command, where
/// what it last published is behind by construction.
impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Attach a bounded decoded-audio observer to `id`'s decoder.
    ///
    /// Attachment is nonblocking and works before, during, or after resource
    /// loading. Only one observer is active for a track at a time.
    pub fn attach_observer<O: AudioObserver>(&self, id: TrackId, observer: O) {
        self.tracks.attach_observer(id, Box::new(observer));
    }

    /// The currently playing track entry, if any.
    ///
    /// Sourced from the navigation cursor (not the player) so the queue
    /// reports `None` after `advance_to_next` runs off the end of the
    /// queue (`RepeatMode::Off` exhaustion). The deck keeps the last item
    /// it played — read its position via [`Self::current_index`] when the
    /// call site needs the last-played index even after queue-end.
    #[must_use]
    pub fn current(&self) -> Option<TrackEntry> {
        self.track(self.navigation.current()?)
    }

    /// Queue position of the track the deck leads; `None` while it leads
    /// none, or one the queue no longer holds.
    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        let id = self.player.current_item()?;
        self.tracks
            .records()
            .iter()
            .position(|record| record.id == id)
    }

    delegate::delegate! {
        to self.tracks.records() {
            /// Whether the queue is empty.
            #[must_use]
            pub fn is_empty(&self) -> bool;
            /// Number of tracks currently in the queue.
            #[must_use]
            pub fn len(&self) -> usize;
        }
        to self.navigation {
            /// Current traversal order.
            #[must_use]
            pub fn playback_order(&self) -> PlaybackOrder;
            /// Current repeat mode.
            #[must_use]
            pub fn repeat_mode(&self) -> RepeatMode;
        }
    }

    /// Lookup a track entry by id.
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<TrackEntry> {
        self.tracks
            .records()
            .iter()
            .find(|record| record.id == id)
            .map(TrackRecord::entry)
    }

    /// The original [`TrackSource`] for `id`, if still queued. Lets callers
    /// rebuild a resource by track identity rather than by queue position.
    #[must_use]
    pub fn track_source(&self, id: TrackId) -> Option<TrackSource<S>> {
        self.tracks.source(id)
    }

    /// Snapshot of all track entries, in queue order.
    #[must_use]
    pub fn tracks(&self) -> Vec<TrackEntry> {
        self.tracks
            .records()
            .iter()
            .map(TrackRecord::entry)
            .collect()
    }
}

/// A handle reads what the queue last published: the queue publishes before
/// it answers a command, so a handle reads its own writes.
impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Attach a bounded decoded-audio observer to `id`'s decoder.
    ///
    /// Attachment is nonblocking and works before, during, or after resource
    /// loading. Only one observer is active for a track at a time.
    pub fn attach_observer<O: AudioObserver>(&self, id: TrackId, observer: O) {
        self.view.attach_observer(id, Box::new(observer));
    }

    /// Queue position of the track the deck leads; `None` while it leads
    /// none, or one the queue no longer holds.
    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        self.view.index_of(self.player.current_item()?)
    }

    delegate::delegate! {
        to self.view {
            /// The currently playing track entry, if any; `None` once the
            /// queue ran off its end.
            #[must_use]
            pub fn current(&self) -> Option<TrackEntry>;
            /// Whether the queue is empty.
            #[must_use]
            pub fn is_empty(&self) -> bool;
            /// Number of tracks currently in the queue.
            #[must_use]
            pub fn len(&self) -> usize;
            /// Current traversal order.
            #[must_use]
            pub fn playback_order(&self) -> PlaybackOrder;
            /// Current repeat mode.
            #[must_use]
            pub fn repeat_mode(&self) -> RepeatMode;
            /// Lookup a track entry by id.
            #[must_use]
            pub fn track(&self, id: TrackId) -> Option<TrackEntry>;
            /// The original [`TrackSource`] for `id`, if still queued.
            #[must_use]
            pub fn track_source(&self, id: TrackId) -> Option<TrackSource<S>>;
            /// Snapshot of all track entries, in queue order.
            #[must_use]
            pub fn tracks(&self) -> Vec<TrackEntry>;
        }
    }
}

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) fn set_action_at_item_end(&mut self, action: ActionAtItemEnd) {
        self.command(|queue| {
            queue.config.set_action_at_item_end(action);
            queue.reconcile_successor();
            queue.announce(QueueEvent::ActionAtItemEndChanged { action });
        });
    }

    pub(crate) fn set_playback_order(&mut self, order: PlaybackOrder) {
        self.command(|queue| {
            let ids = queue.track_ids();
            queue.navigation.set_playback_order(order, &ids);
            queue.reconcile_successor();
            queue.announce(QueueEvent::PlaybackOrderChanged { order });
        });
    }

    pub(crate) fn set_repeat(&mut self, mode: RepeatMode) {
        self.command(|queue| {
            queue.navigation.set_repeat(mode);
            queue.reconcile_successor();
            queue.announce(QueueEvent::RepeatModeChanged {
                mode: map_repeat_mode(mode),
            });
        });
    }
}

const fn map_repeat_mode(mode: RepeatMode) -> QueueRepeatMode {
    match mode {
        RepeatMode::Off => QueueRepeatMode::Off,
        RepeatMode::One => QueueRepeatMode::One,
        RepeatMode::All => QueueRepeatMode::All,
    }
}
