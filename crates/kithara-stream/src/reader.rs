use std::io::{self, Read, Seek, SeekFrom};

use kithara_platform::{sync::Arc, time::Duration};

use crate::{ByteMap, SharedStream, StreamType, hooks::BoxedEventSink};

/// What a read does when the bytes it asked for have not arrived.
///
/// The choice belongs to the reader, not to the stream: one session can be
/// building a decoder off the real-time path and want to wait, while another
/// is feeding the produce core and must never block.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum WaitMode {
    /// Real-time / cooperative-yield probe: bounded wait, returns without
    /// blocking so the caller can park and re-tick.
    #[default]
    Probe,
    /// Off-real-time: park event-driven until the range resolves.
    Block,
}

impl WaitMode {
    pub(crate) fn timeout(self, probe: Duration) -> Option<Duration> {
        match self {
            Self::Probe => Some(probe),
            Self::Block => None,
        }
    }
}

/// Where a reader starts in the stream and how it waits there.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, bon::Builder)]
#[non_exhaustive]
pub struct ReaderHint {
    /// Absolute byte the reader treats as its own position zero. A decoder
    /// rebuilt mid-stream is handed the segment it starts at, and expects to
    /// address it from 0.
    #[builder(default)]
    pub base_offset: u64,
    #[builder(default)]
    pub wait: WaitMode,
}

/// One reading session over a stream.
///
/// Everything the decode path needs to build a decoder comes from here: the
/// bytes through `Read + Seek`, and the three facts a decoder has to be
/// configured with. Before this, the caller read those three off the stream
/// and constructed the reader separately, which only worked while there was
/// exactly one reader and it was the whole stream.
pub trait SessionReader: Read + Seek + Send + Sync {
    /// Bytes this session can address, from ITS zero — already net of where
    /// the session starts. `None` when the stream length is not known yet.
    fn byte_len(&self) -> Option<u64>;

    /// Segment map, for decoders that demux segment by segment.
    fn byte_map(&self) -> Option<Arc<dyn ByteMap>>;

    /// Reader-side event sink, taken once per session.
    fn take_event_sink(&mut self) -> Option<BoxedEventSink>;
}

/// A [`SessionReader`] that reads a stream through a private base offset.
///
/// Positions it reports and accepts are relative to `base_offset`, so a
/// decoder rebuilt at a mid-stream boundary addresses its segment from 0
/// while the bytes come from wherever the stream actually keeps them.
pub struct CursorReader<T: StreamType> {
    shared: SharedStream<T>,
    base_offset: u64,
    wait: WaitMode,
}

impl<T: StreamType> CursorReader<T> {
    fn new(shared: SharedStream<T>, hint: ReaderHint) -> Self {
        let reader = Self {
            shared,
            base_offset: hint.base_offset,
            wait: hint.wait,
        };
        // Aim the stream at the session's start before anyone reads: the
        // decoder's first read has to land on its own segment, and its first
        // `seek` may never come. Position math only, never the priming seek:
        // opening a session must not wait for bytes nobody has asked for yet.
        // A target the stream cannot resolve yet is not an error here — the
        // read that follows discovers it.
        let _ = reader.shared.probe_seek(SeekFrom::Start(hint.base_offset));
        reader
    }
}

impl<T: StreamType> SessionReader for CursorReader<T> {
    fn byte_len(&self) -> Option<u64> {
        self.shared
            .len()
            .map(|len| len.saturating_sub(self.base_offset))
    }

    fn byte_map(&self) -> Option<Arc<dyn ByteMap>> {
        self.shared.byte_map()
    }

    fn take_event_sink(&mut self) -> Option<BoxedEventSink> {
        self.shared.take_reader_event_sink()
    }
}

impl<T: StreamType> Read for CursorReader<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let from = self.shared.position();
        let count = match self.wait {
            WaitMode::Probe => self.shared.probe_read_from(from, buf)?,
            WaitMode::Block => self.shared.blocking_read_from(from, buf)?,
        };
        // Crediting what this session consumed is what makes the stream's
        // cursor the track's position — a separate act from the read itself.
        self.shared.advance(count as u64);
        Ok(count)
    }
}

impl<T: StreamType> Seek for CursorReader<T> {
    /// Seeks the way the session reads: a `Probe` session feeds the produce
    /// core, where the off-RT adapter's inline priming is not allowed to spin;
    /// a `Block` session is already off that core and may prime.
    ///
    /// `Start` is the only variant the base offset applies to going in —
    /// `Current` and `End` are already expressed against the stream's own
    /// cursor and end. Coming back out, all three are reported from the
    /// session's zero.
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => SeekFrom::Start(self.base_offset.saturating_add(p)),
            other => other,
        };
        let from = self.shared.position();
        let landed = match self.wait {
            WaitMode::Probe => self.shared.probe_seek_from(from, target)?,
            WaitMode::Block => self.shared.blocking_seek_from(from, target)?,
        };
        self.shared.set_position(landed);
        Ok(landed.saturating_sub(self.base_offset))
    }
}

impl<T: StreamType> SharedStream<T> {
    /// Open a reading session over this stream.
    ///
    /// The one way the decode path gets a reader, for every stream type —
    /// what differs between a file and a segmented stream is inside the
    /// source, not in how a reader over it is made.
    #[must_use]
    pub fn open_reader(&self, hint: ReaderHint) -> CursorReader<T> {
        CursorReader::new(self.clone(), hint)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroUsize,
        ops::Range,
        sync::atomic::{AtomicU64, Ordering},
    };

    use kithara_platform::sync::Mutex;
    use kithara_storage::WaitOutcome;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        Activity, PlayheadRead, PlayheadState, PlayheadWrite, ReadOutcome, SeekControl,
        SeekObserve, Source, SourceError, SourcePhase, Stream, StreamResult, TimelineState,
    };

    const LEN: u64 = 100;

    struct FlatSource {
        position: Arc<AtomicU64>,
        seek: Arc<TimelineState>,
        playhead: Arc<PlayheadState>,
        /// Timeout each `wait_range` was given. `Some` is a bounded probe,
        /// `None` is an unbounded park — which is how a reader's wait mode
        /// becomes visible at the source boundary.
        waits: Arc<Mutex<Vec<Option<Duration>>>>,
        /// Offset each `read_at` was called with — where the session that
        /// issued it believes it is.
        reads: Arc<Mutex<Vec<u64>>>,
    }

    impl Source for FlatSource {
        fn playhead_read(&self) -> Arc<dyn PlayheadRead> {
            Arc::clone(&self.playhead) as Arc<dyn PlayheadRead>
        }
        fn playhead_write(&self) -> Arc<dyn PlayheadWrite> {
            Arc::clone(&self.playhead) as Arc<dyn PlayheadWrite>
        }
        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }
        fn seek_control(&self) -> Arc<dyn SeekControl> {
            Arc::clone(&self.seek) as Arc<dyn SeekControl>
        }
        fn activity(&self) -> Arc<dyn Activity> {
            Arc::clone(&self.seek) as Arc<dyn Activity>
        }
        fn wait_range(
            &mut self,
            _range: Range<u64>,
            timeout: Option<Duration>,
        ) -> StreamResult<WaitOutcome> {
            self.waits.lock().push(timeout);
            Ok(WaitOutcome::Ready)
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> StreamResult<ReadOutcome> {
            self.reads.lock().push(offset);
            Ok(NonZeroUsize::new(buf.len()).map_or(ReadOutcome::Eof, ReadOutcome::Bytes))
        }
        /// Never resident, so a priming seek actually reaches `wait_range`
        /// instead of taking the cache-hit short circuit.
        fn phase_at(&self, _range: Range<u64>) -> SourcePhase {
            SourcePhase::Waiting
        }
        fn len(&self) -> Option<u64> {
            Some(LEN)
        }
        fn position(&self) -> u64 {
            self.position.load(Ordering::Acquire)
        }
        fn advance(&self, n: u64) {
            self.position.fetch_add(n, Ordering::AcqRel);
        }
        fn set_position(&self, pos: u64) {
            self.position.store(pos, Ordering::Release);
        }
    }

    struct FlatType;

    impl StreamType for FlatType {
        type Config = ();
        type Events = ();
        type Source = FlatSource;

        async fn create(_config: Self::Config) -> Result<Self::Source, SourceError> {
            Err(SourceError::other(io::Error::other(
                "not used in unit tests",
            )))
        }
    }

    struct Probe {
        stream: SharedStream<FlatType>,
        waits: Arc<Mutex<Vec<Option<Duration>>>>,
        reads: Arc<Mutex<Vec<u64>>>,
    }

    fn probe() -> Probe {
        let waits = Arc::new(Mutex::new(Vec::new()));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let stream = SharedStream::new(Stream::<FlatType> {
            source: FlatSource {
                position: Arc::new(AtomicU64::new(0)),
                seek: Arc::new(TimelineState::new()),
                playhead: Arc::new(PlayheadState::new()),
                waits: Arc::clone(&waits),
                reads: Arc::clone(&reads),
            },
        });
        Probe {
            stream,
            waits,
            reads,
        }
    }

    fn reader_at(base: u64) -> (SharedStream<FlatType>, CursorReader<FlatType>) {
        let shared = probe().stream;
        let reader = shared.open_reader(ReaderHint::builder().base_offset(base).build());
        (shared, reader)
    }

    /// A decoder rebuilt mid-stream may read before it ever seeks, so opening
    /// the session has to leave the stream at the session's start.
    #[kithara::test]
    fn opening_a_session_aims_the_stream_at_its_start() {
        let (shared, _reader) = reader_at(40);
        assert_eq!(shared.position(), 40);
    }

    /// `Start` is the one variant the base applies to going in, and every
    /// variant reports back from the session's zero.
    #[kithara::test]
    fn start_is_rebased_both_ways() {
        let (shared, mut reader) = reader_at(40);

        assert_eq!(reader.seek(SeekFrom::Start(10)).expect("seek"), 10);
        assert_eq!(
            shared.position(),
            50,
            "the stream moved to the absolute byte"
        );
    }

    /// `Current` is already expressed against the stream's own cursor, so it
    /// passes through untouched — only the answer is rebased.
    #[kithara::test]
    fn current_passes_through_and_only_the_answer_is_rebased() {
        let (shared, mut reader) = reader_at(40);

        assert_eq!(reader.seek(SeekFrom::Current(10)).expect("seek"), 10);
        assert_eq!(shared.position(), 50);
    }

    /// `End` likewise: the end belongs to the stream, not to the session.
    #[kithara::test]
    fn end_is_the_streams_end_reported_from_the_sessions_zero() {
        let (_shared, mut reader) = reader_at(40);

        assert_eq!(reader.seek(SeekFrom::End(0)).expect("seek"), LEN - 40);
    }

    /// A session starting past the end reports zero rather than wrapping —
    /// the reverse map saturates.
    #[kithara::test]
    fn a_session_past_the_end_reports_zero_not_a_wrapped_position() {
        let (_shared, mut reader) = reader_at(LEN + 10);

        assert_eq!(reader.seek(SeekFrom::Start(0)).expect("seek"), 0);
    }

    /// Whether a read waits belongs to the session, not to the stream: one
    /// session can be building a decoder off the real-time path and want to
    /// park, while another feeds the produce core over the same stream and
    /// must never block. Two readers, one stream, two answers.
    #[kithara::test]
    fn two_sessions_over_one_stream_wait_differently() {
        let Probe { stream, waits, .. } = probe();
        let mut probing = stream.open_reader(ReaderHint::builder().build());
        let mut block = stream.open_reader(ReaderHint::builder().wait(WaitMode::Block).build());
        let mut buf = [0_u8; 8];

        waits.lock().clear();
        let _ = probing.read(&mut buf);
        let probed = waits.lock().clone();
        assert!(
            !probed.is_empty() && probed.iter().all(Option::is_some),
            "a probe session waits on a budget: {probed:?}"
        );

        waits.lock().clear();
        let _ = block.read(&mut buf);
        let blocked = waits.lock().clone();
        assert!(
            !blocked.is_empty() && blocked.iter().all(Option::is_none),
            "a block session parks until the range resolves: {blocked:?}"
        );
    }

    /// A seek waits the same way its session reads. A block session is off the
    /// produce core, so its seek may prime the target range; a probe session
    /// does position math only and discovers lateness at the read.
    #[kithara::test]
    fn a_seek_primes_only_for_the_session_that_waits() {
        let Probe { stream, waits, .. } = probe();
        let mut probing = stream.open_reader(ReaderHint::builder().build());
        let mut block = stream.open_reader(ReaderHint::builder().wait(WaitMode::Block).build());

        waits.lock().clear();
        probing.seek(SeekFrom::Start(10)).expect("probe seek");
        assert!(waits.lock().is_empty(), "a probe seek does not prime");

        waits.lock().clear();
        block.seek(SeekFrom::Start(10)).expect("block seek");
        let primed = waits.lock().clone();
        assert!(
            !primed.is_empty() && primed.iter().all(Option::is_none),
            "a block seek primes its target range: {primed:?}"
        );
    }

    /// What a session consumes is credited to the stream: that is what makes
    /// its cursor the track's position, and the stream follows byte for byte.
    #[kithara::test]
    fn what_a_session_consumes_becomes_the_track_position() {
        let Probe { stream, reads, .. } = probe();
        let mut session = stream.open_reader(ReaderHint::builder().base_offset(40).build());
        let mut buf = [0_u8; 8];

        assert_eq!(session.read(&mut buf).expect("session read"), 8);
        assert_eq!(session.read(&mut buf).expect("session read again"), 8);

        assert_eq!(*reads.lock(), vec![40, 48]);
        assert_eq!(
            stream.position(),
            56,
            "the track followed what the session consumed"
        );
    }

    /// What the decoder is told about length is what IT can address, not what
    /// the stream holds.
    #[kithara::test]
    fn byte_len_is_net_of_where_the_session_starts() {
        let (_shared, reader) = reader_at(40);
        assert_eq!(reader.byte_len(), Some(LEN - 40));

        let (_shared, past_end) = reader_at(LEN + 10);
        assert_eq!(
            past_end.byte_len(),
            Some(0),
            "a session past the end addresses nothing"
        );
    }
}
