use std::num::NonZeroUsize;

use kithara_audio::SeekBegin;
use kithara_command::{Batch, ChannelConfig, Inbox, SendError, Sender, Seq, When, channel};
use kithara_effects::eq::EqLayout;
use kithara_events::TrackId;
use kithara_output::LiveOutput;
use kithara_platform::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use kithara_signal::AudioSpec;
use kithara_warp::{RenderReader, RenderSnapshot};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Observer, Producer, Split},
};

use super::PlaybackShared;
use crate::{
    bridge::{DeckPart, DeckProtocol, PlayerNotification},
    rt::track::PlayerTrack,
};

/// RT-owned channel halves and playback atomics for one player node.
#[non_exhaustive]
pub struct NodeInputs {
    pub(crate) playback: Arc<PlaybackShared>,
    pub(crate) deck: Inbox<DeckProtocol>,
    pub(crate) notif_tx: HeapProd<PlayerNotification>,
    pub(crate) trash_tx: HeapProd<DeckTrash>,
}

/// What a deck's audio thread hands back to be dropped off it.
pub enum DeckTrash {
    /// A track the deck no longer holds.
    Track(PlayerTrack),
    /// An EQ layout the deck's equaliser displaced.
    Eq(Box<EqLayout>),
}

/// Producer for interleaved stereo mix samples and their drop count.
#[non_exhaustive]
pub struct MixTapWriter {
    pub(crate) drops: Arc<AtomicU64>,
    pub(crate) samples: HeapProd<f32>,
}

impl MixTapWriter {
    #[must_use]
    pub fn new(samples: HeapProd<f32>, drops: Arc<AtomicU64>) -> Self {
        Self { drops, samples }
    }
}

impl From<MixTapWriter> for (HeapProd<f32>, Arc<AtomicU64>) {
    fn from(writer: MixTapWriter) -> Self {
        (writer.samples, writer.drops)
    }
}

impl LiveOutput for MixTapWriter {
    fn reconfigure(&mut self, _spec: AudioSpec) {}

    fn write_stereo(&mut self, frames: usize, left: &[f32], right: &[f32]) {
        let stereo = 2;
        let writable = frames
            .min(left.len())
            .min(right.len())
            .min(self.samples.vacant_len() / stereo);
        let pushed = self.samples.push_iter(
            left[..writable]
                .iter()
                .zip(&right[..writable])
                .flat_map(|(&left, &right)| [left, right]),
        );
        let dropped = frames.saturating_mul(stereo).saturating_sub(pushed);
        if dropped > 0 {
            self.drops.fetch_add(
                u64::try_from(dropped).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
    }
}

/// Control-owned channel halves and shared controls for one allocated slot.
#[non_exhaustive]
pub struct SlotControl {
    pub playback: Arc<PlaybackShared>,
    pub notif_rx: HeapCons<PlayerNotification>,
    pub trash_rx: HeapCons<DeckTrash>,
    pub deck: Sender<DeckProtocol>,
    render: RenderBindings,
    seek: SeekBindings,
}

#[derive(Default)]
struct SeekBindings(Vec<SeekBinding>);

type SeekBinding = (TrackId, Arc<dyn SeekBegin>);

#[derive(Default)]
struct RenderBindings(Vec<RenderBinding>);

type RenderBinding = (TrackId, RenderReader);

impl SlotControl {
    /// Begin a seek on every track this slot holds, off the audio thread.
    pub fn begin_seek(&self, position: Duration) {
        for (_, handle) in &self.seek.0 {
            handle.begin(position);
        }
    }

    pub(crate) fn bind_render(&mut self, item_id: TrackId, reader: RenderReader) {
        self.render.0.push((item_id, reader));
    }

    /// Record the control half of a track's seek path.
    pub fn bind_seek(&mut self, item_id: TrackId, handle: Arc<dyn SeekBegin>) {
        self.seek.0.push((item_id, handle));
    }

    /// Sends `part` to apply at the start of the deck's next block.
    ///
    /// # Errors
    ///
    /// Returns the batch whole when the deck's capacity of batches is in flight.
    pub fn send(&mut self, part: DeckPart) -> Result<Seq, SendError<DeckProtocol>> {
        self.send_batch(vec![part])
    }

    /// Sends `commands` to apply together, in order, at the start of the deck's next block: the
    /// deck admits all of them or none.
    ///
    /// The receipts that came back since the last send are dropped first: they return the
    /// credits, and every batch for the next block applies. A resource crossing to the audio
    /// thread leaves its seek handle and render reader here once the deck admits it, since
    /// seeking takes locks; both unbind when the resource returns as trash.
    ///
    /// # Errors
    ///
    /// Returns the batch whole when the deck's capacity of batches is in flight.
    pub fn send_batch(&mut self, commands: Vec<DeckPart>) -> Result<Seq, SendError<DeckProtocol>> {
        let bindings: Vec<_> = commands
            .iter()
            .filter_map(|command| match command {
                DeckPart::Attach { resource, item_id } => {
                    Some((*item_id, resource.seek_handle(), resource.render_reader()))
                }
                _ => None,
            })
            .collect();
        self.deck.receipts().for_each(drop);
        let seq = self.deck.send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands,
            },
        )?;
        for (item_id, seek, render) in bindings {
            if let Some(seek) = seek {
                self.bind_seek(item_id, seek);
            }
            if let Some(render) = render {
                self.bind_render(item_id, render);
            }
        }
        Ok(seq)
    }

    /// The newest render any bound track has published.
    #[must_use]
    pub fn latest_render_snapshot(&self) -> Option<RenderSnapshot> {
        self.render
            .0
            .iter()
            .filter_map(|(_, reader)| reader.load())
            .max_by_key(|snapshot| {
                let context = snapshot.context();
                (
                    u64::from(context.output().session_epoch()),
                    i64::from(context.output().output_frames().end),
                )
            })
    }

    /// Forget the exact render reader returned by the processor.
    pub fn unbind_render(&mut self, item_id: TrackId, reader: &RenderReader) {
        self.render
            .0
            .retain(|(bound_id, bound_reader)| *bound_id != item_id || bound_reader != reader);
    }

    /// Forget the exact resource generation returned by the processor.
    pub fn unbind_seek(&mut self, item_id: TrackId, handle: &Arc<dyn SeekBegin>) {
        self.seek.0.retain(|(bound_id, bound_handle)| {
            *bound_id != item_id || !Arc::ptr_eq(bound_handle, handle)
        });
    }
}

#[must_use]
pub fn slot_channels() -> (NodeInputs, SlotControl) {
    const DECK_CAPACITY: NonZeroUsize = match NonZeroUsize::new(32) {
        Some(capacity) => capacity,
        None => unreachable!(),
    };
    const NOTIFICATION_CAPACITY: usize = 32;
    const TRASH_CAPACITY: usize = 64;

    let (sender, inbox) =
        channel::<DeckProtocol>(ChannelConfig::builder().capacity(DECK_CAPACITY).build());
    let (notif_tx, notif_rx) = HeapRb::<PlayerNotification>::new(NOTIFICATION_CAPACITY).split();
    let (trash_tx, trash_rx) = HeapRb::<DeckTrash>::new(TRASH_CAPACITY).split();
    let playback = Arc::new(PlaybackShared::default());

    let inputs = NodeInputs {
        deck: inbox,
        notif_tx,
        trash_tx,
        playback: Arc::clone(&playback),
    };
    let control = SlotControl {
        playback,
        notif_rx,
        trash_rx,
        deck: sender,
        seek: SeekBindings::default(),
        render: RenderBindings::default(),
    };
    (inputs, control)
}
