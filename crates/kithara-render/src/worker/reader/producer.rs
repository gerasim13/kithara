use kithara_audio::AudioEvent;
use kithara_events::EventBus;
use kithara_platform::{
    sync::{Arc, ThreadGate, WaitGate},
    time::Duration,
};
use kithara_signal::AudioSpec;
use kithara_worker::Wake;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use triple_buffer::{Output, triple_buffer};

use super::lead::PcmLead;
use crate::{PcmPacket, PcmReceiver};

pub(in crate::worker) struct PcmProducer {
    pub(in crate::worker) forward: HeapProd<PcmPacket>,
    pub(in crate::worker) reverse: HeapCons<PcmPacket>,
    pub(in crate::worker) lead: PcmLead,
    pub(in crate::worker) playing: Output<bool>,
    pub(super) ready: FinalWake,
    pub(in crate::worker) events: EventBus,
    pub(in crate::worker) output_pending: bool,
}

pub(super) struct FinalWake(pub(super) Option<Arc<ThreadGate>>);

impl Drop for FinalWake {
    fn drop(&mut self) {
        if let Some(ready) = &self.0 {
            ready.signal();
        }
    }
}

impl PcmProducer {
    /// The reverse ring fits the forward slots, the reader-held packet, and one
    /// worker push between recycles, so returning an in-flight packet never fails.
    pub(in crate::worker) fn new(
        lead: PcmLead,
        block_on_underrun: bool,
        wake: Wake,
        spec: AudioSpec,
        frontier: Duration,
        events: EventBus,
    ) -> (PcmReceiver, Self) {
        let (forward_tx, forward_rx) = HeapRb::new(lead.slots()).split();
        let (reverse_tx, reverse_rx) = HeapRb::new(lead.slots() + 2).split();
        let (playing_tx, playing_rx) = triple_buffer(&false);
        let ready = block_on_underrun.then(|| Arc::new(ThreadGate::default()));
        (
            PcmReceiver {
                forward: forward_rx,
                reverse: reverse_tx,
                playing: playing_tx,
                ready: ready.as_ref().map(Arc::downgrade),
                wake,
                spec,
                frontier,
            },
            Self {
                forward: forward_tx,
                reverse: reverse_rx,
                lead,
                playing: playing_rx,
                ready: FinalWake(ready),
                events,
                output_pending: false,
            },
        )
    }

    /// Admit one packet into the lead, or hand it back while the lead or the
    /// ring is full.
    /// Markers count one frame so every in-flight packet occupies the lead.
    pub(in crate::worker) fn push(&mut self, packet: PcmPacket) -> Result<(), PcmPacket> {
        let frames = match &packet {
            PcmPacket::Chunk(chunk) => chunk.frames().max(1),
            PcmPacket::Failed { .. } => 1,
        };
        let queued = self.forward.occupied_len();
        if !self.lead.admits(frames, queued) {
            return Err(packet);
        }
        self.forward.try_push(packet)?;
        self.lead.add(frames, queued);
        Ok(())
    }

    /// Take back one packet the reader returned.
    pub(in crate::worker) fn reclaim(&mut self) -> Option<PcmPacket> {
        let packet = self.reverse.try_pop()?;
        self.lead.count_return();
        Some(packet)
    }

    /// A whole quantum no longer fits the lead.
    pub(in crate::worker) fn is_full(&self) -> bool {
        self.forward.is_full() || !self.lead.admits_quantum(self.forward.occupied_len())
    }

    pub(in crate::worker) fn signal(&mut self) {
        self.output_pending = true;
        if let Some(ready) = &self.ready.0 {
            ready.signal();
        }
    }

    pub(in crate::worker) fn flush(&mut self) {
        if std::mem::take(&mut self.output_pending) {
            self.events.publish(AudioEvent::OutputAvailable);
        }
    }
}
