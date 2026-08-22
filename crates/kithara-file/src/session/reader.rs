use std::sync::atomic::{AtomicU64, Ordering};

use kithara_events::{DeferredBus, EventBus, FileEvent};
use kithara_platform::sync::Arc;
use kithara_stream::{ReaderChunkSignal, ReaderEventSink, ReaderSeekSignal};

use crate::coord::FileCoord;

pub(crate) struct FileReaderEventSink {
    coord: Arc<FileCoord>,
    seek_epoch_handle: Arc<AtomicU64>,
    bus: DeferredBus<FileEvent>,
    initial_seek_published: bool,
    /// See `HlsReaderEventSink::initial_cursor` — same recreate-after-
    /// seek-failure scenario.
    initial_cursor: u64,
    last_cursor: u64,
}

impl FileReaderEventSink {
    pub(crate) fn new(
        bus: EventBus,
        coord: Arc<FileCoord>,
        seek_epoch_handle: Arc<AtomicU64>,
        event_capacity: usize,
    ) -> Self {
        let last_cursor = coord.position();
        Self {
            bus: DeferredBus::new(bus, event_capacity),
            coord,
            last_cursor,
            seek_epoch_handle,
            initial_cursor: last_cursor,
            initial_seek_published: false,
        }
    }

    fn publish_initial_seek(&mut self, cursor: u64) {
        if self.initial_seek_published {
            return;
        }
        self.initial_seek_published = true;
        let seek_epoch = self.seek_epoch_handle.load(Ordering::Acquire);
        if seek_epoch == 0 {
            return;
        }
        self.bus.enqueue(FileEvent::ReaderSeek {
            seek_epoch,
            from_offset: self.initial_cursor,
            to_offset: cursor,
        });
    }
}

impl ReaderEventSink for FileReaderEventSink {
    fn flush(&mut self) {
        self.bus.flush();
    }

    fn on_chunk(&mut self, signal: ReaderChunkSignal) {
        if !matches!(signal, ReaderChunkSignal::Chunk) {
            return;
        }
        let cursor = self.coord.position();
        self.publish_initial_seek(cursor);
        self.last_cursor = cursor;
        self.bus.enqueue(FileEvent::ReadProgress {
            position: cursor,
            total: self.coord.total_bytes(),
        });
    }

    fn on_seek(&mut self, signal: ReaderSeekSignal) {
        self.initial_seek_published = true;
        let ReaderSeekSignal::Landed { landed_byte, .. } = signal else {
            return;
        };
        let Some(to) = landed_byte else {
            return;
        };
        let from = self.last_cursor;
        self.last_cursor = to;
        let seek_epoch = self.seek_epoch_handle.load(Ordering::Acquire);
        self.bus.enqueue(FileEvent::ReaderSeek {
            seek_epoch,
            from_offset: from,
            to_offset: to,
        });
    }
}

#[cfg(test)]
mod tests {
    use kithara_events::{BusEvent, Event};
    use kithara_stream::{PlayheadState, SeekState};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::config::DEFAULT_READER_EVENT_CAPACITY;

    fn sink(bus: EventBus, event_capacity: usize) -> FileReaderEventSink {
        let coord = Arc::new(FileCoord::new(
            Arc::new(PlayheadState::new()),
            Arc::new(SeekState::new()),
        ));
        FileReaderEventSink::new(bus, coord, Arc::new(AtomicU64::new(0)), event_capacity)
    }

    fn burst(sink: &mut FileReaderEventSink, chunks: usize) {
        for _ in 0..chunks {
            sink.on_chunk(ReaderChunkSignal::Chunk);
        }
        sink.flush();
    }

    fn dropped_in(bus: &EventBus, chunks: usize, event_capacity: usize) -> u64 {
        let mut rx = bus.subscribe();
        burst(&mut sink(bus.clone(), event_capacity), chunks);
        let mut dropped = 0;
        while let Ok(envelope) = rx.try_recv() {
            if let Event::Bus(BusEvent::Overflow { dropped: count, .. }) = envelope.event {
                dropped += count;
            }
        }
        dropped
    }

    /// The ring depth is the caller's, not a crate constant: a one-slot ring
    /// keeps the first chunk of a burst and reports the rest as dropped.
    #[kithara::test]
    fn a_one_slot_ring_drops_the_rest_of_the_burst() {
        assert_eq!(dropped_in(&EventBus::new(16), 3, 1), 2);
    }

    #[kithara::test]
    fn the_default_ring_absorbs_the_same_burst() {
        let dropped = dropped_in(&EventBus::new(16), 3, DEFAULT_READER_EVENT_CAPACITY);

        assert_eq!(dropped, 0);
    }
}
