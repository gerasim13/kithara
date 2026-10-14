use std::{num::NonZeroU32, sync::Mutex};

use kithara_platform::sync::Arc;
use kithara_stream::{
    BoxedEventSink, PendingReason, ReaderChunkSignal, ReaderEventSink, ReaderSeekSignal,
};
use kithara_test_fixtures::{mock_fixtures::zero_packet, unit_fixtures::trim_silence};
use kithara_test_utils::kithara;

use super::{test_stub_codec::ConstFrameCodec, *};
use crate::{
    demuxer::{DemuxOutcome, DemuxSeekOutcome, Frame, TrackInfo},
    traits::Decoder,
};

#[derive(Default)]
struct CallLog {
    chunks: Vec<&'static str>,
    seeks: Vec<&'static str>,
}

struct LoggingHooks {
    log: Arc<Mutex<CallLog>>,
}

impl ReaderEventSink for LoggingHooks {
    fn on_chunk(&mut self, signal: ReaderChunkSignal) {
        let tag = match signal {
            ReaderChunkSignal::Chunk => "chunk",
            ReaderChunkSignal::Pending(_) => "pending",
            ReaderChunkSignal::Eof => "eof",
            _ => "other",
        };
        self.log.lock().unwrap().chunks.push(tag);
    }

    fn on_seek(&mut self, signal: ReaderSeekSignal) {
        let tag = match signal {
            ReaderSeekSignal::Landed { .. } => "landed",
            ReaderSeekSignal::PastEof => "past_eof",
            _ => "other",
        };
        self.log.lock().unwrap().seeks.push(tag);
    }
}

/// Owned variant of `DemuxOutcome` used to seed the stub. Kept
/// separate so the borrowed `DemuxOutcome<'_>` returned by
/// `next_frame` can be backed by the stub's own buffer.
enum StubOutcome {
    Frame { pts: Duration, duration: Duration },
    Pending(PendingReason),
}

/// Stub demuxer + codec pair driven by canned outcomes. Exists only so
/// hook tests can construct a `ComposedDecoder` without needing a real
/// container/codec.
struct StubDemuxer {
    track: TrackInfo,
    held: Vec<u8>,
    next: Vec<StubOutcome>,
    seek: Vec<DemuxSeekOutcome>,
}

impl StubDemuxer {
    fn with_outcomes(
        zero_packet: &[u8],
        next: Vec<StubOutcome>,
        seek: Vec<DemuxSeekOutcome>,
    ) -> Self {
        Self {
            next,
            seek,
            track: empty_track(),
            held: zero_packet.to_vec(),
        }
    }
}

impl Demuxer for StubDemuxer {
    fn duration(&self) -> Option<Duration> {
        None
    }
    fn next_frame(&mut self) -> DecodeResult<DemuxOutcome<'_>> {
        match self.next.pop() {
            Some(StubOutcome::Frame { pts, duration }) => Ok(DemuxOutcome::Frame(Frame {
                pts,
                duration,
                data: &self.held,
                packet_desc: &[],
            })),
            Some(StubOutcome::Pending(reason)) => Ok(DemuxOutcome::Pending(reason)),
            None => Ok(DemuxOutcome::Eof),
        }
    }
    fn seek(
        &mut self,
        _pos: Duration,
        _priming: crate::codec::CodecPriming,
    ) -> DecodeResult<DemuxSeekOutcome> {
        Ok(self.seek.pop().unwrap_or(DemuxSeekOutcome::PastEof {
            duration: Duration::ZERO,
        }))
    }
    fn track_info(&self) -> &TrackInfo {
        &self.track
    }
}

fn empty_track() -> TrackInfo {
    TrackInfo {
        codec: AudioCodec::Flac,
        sample_rate: 44_100,
        channels: 2,
        extra_data: Vec::new(),
        duration: None,
        gapless: None,
    }
}

fn build(
    trim_silence: &[f32],
    demuxer: StubDemuxer,
    log: Arc<Mutex<CallLog>>,
) -> ComposedDecoder<StubDemuxer, ConstFrameCodec, crate::test_pools::TestPools> {
    let codec = ConstFrameCodec::new(
        trim_silence.to_vec(),
        AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate")),
        1,
    );
    let hooks: BoxedEventSink = Box::new(LoggingHooks { log });
    ComposedDecoder::new(
        demuxer,
        codec,
        DecoderRuntime {
            hooks: Some(hooks),
            ..DecoderRuntime::for_test()
        },
    )
}

#[kithara::test]
#[case::chunk_signal(
    StubOutcome::Frame {
        pts: Duration::ZERO,
        duration: Duration::from_millis(20),
    },
    "chunk"
)]
#[case::pending_signal(
    StubOutcome::Pending(PendingReason::NotReady(NotReadyCause::SourcePending)),
    "pending"
)]
fn next_chunk_emits_signal(
    #[case] outcome: StubOutcome,
    #[case] expected_signal: &str,
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let log = Arc::new(Mutex::new(CallLog::default()));
    let demuxer = StubDemuxer::with_outcomes(zero_packet, vec![outcome], Vec::new());
    let mut decoder = build(&trim_silence, demuxer, Arc::clone(&log));
    let _ = decoder.next_chunk().unwrap();
    assert_eq!(log.lock().unwrap().chunks, vec![expected_signal]);
}

#[kithara::test]
fn a_direct_pending_rearms_prefill_for_deferred_pcm(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let log = Arc::new(Mutex::new(CallLog::default()));
    let reason = PendingReason::NotReady(NotReadyCause::SourcePending);
    let frame_duration = Duration::from_micros(23);
    let demuxer = StubDemuxer::with_outcomes(
        zero_packet,
        vec![
            StubOutcome::Frame {
                pts: Duration::from_millis(20),
                duration: frame_duration,
            },
            StubOutcome::Pending(reason),
            StubOutcome::Frame {
                pts: Duration::ZERO,
                duration: frame_duration,
            },
        ],
        Vec::new(),
    );
    let mut decoder = build(&trim_silence, demuxer, Arc::clone(&log));
    assert!(matches!(
        decoder.next_chunk().expect("initial chunk"),
        DecoderChunkOutcome::Chunk(_)
    ));
    decoder.prepare_next_chunk();
    assert!(matches!(
        decoder.next_chunk_prepared().expect("direct pending"),
        DecoderChunkOutcome::Pending(actual) if actual == reason
    ));
    assert_eq!(decoder.demuxer.next.len(), 1);
    decoder.prepare_next_chunk();
    assert!(decoder.has_prepared_result());
    assert!(
        decoder.demuxer.next.is_empty(),
        "deferred preparation owns the next PCM"
    );
    decoder.prepare_next_chunk();
    let DecoderChunkOutcome::Chunk(output) = decoder.next_chunk_prepared().expect("deferred PCM")
    else {
        panic!("deferred PCM must remain available");
    };
    assert_eq!(output.meta.timestamp, Duration::from_millis(20));
    assert_eq!(output.frames(), 1);
    assert_eq!(
        log.lock().expect("hook log").chunks,
        ["chunk", "pending", "chunk"]
    );
}

#[kithara::test]
fn zero_frame_codec_yields_pending_before_demux_eof(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let outcomes = (0..=consts::ZERO_FRAME_BUDGET)
        .map(|index| StubOutcome::Frame {
            pts: Duration::from_millis(u64::from(index)),
            duration: Duration::from_millis(1),
        })
        .collect();
    let demuxer = StubDemuxer::with_outcomes(zero_packet, outcomes, Vec::new());
    let codec = ConstFrameCodec::new(
        trim_silence.clone(),
        AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate")),
        0,
    );
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let outcome = decoder.next_chunk().expect("zero-frame decode");

    assert!(matches!(
        outcome,
        DecoderChunkOutcome::Pending(PendingReason::NotReady(NotReadyCause::SourcePending))
    ));
    assert_eq!(decoder.demuxer.next.len(), 1);
}

#[kithara::test]
#[case::landed(
    DemuxSeekOutcome::Landed {
        landed_at: Duration::from_secs(1),
        landed_byte: Some(123),
        preroll: crate::demuxer::PrerollHint::NotNeeded,
    },
    Duration::from_secs(1),
    "landed"
)]
#[case::past_eof(
    DemuxSeekOutcome::PastEof {
        duration: Duration::from_secs(10),
    },
    Duration::from_secs(15),
    "past_eof"
)]
fn seek_emits_signal(
    #[case] outcome: DemuxSeekOutcome,
    #[case] target: Duration,
    #[case] expected_signal: &str,
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let log = Arc::new(Mutex::new(CallLog::default()));
    let demuxer = StubDemuxer::with_outcomes(zero_packet, Vec::new(), vec![outcome]);
    let mut decoder = build(&trim_silence, demuxer, Arc::clone(&log));
    let _ = decoder.seek(target).unwrap();
    assert_eq!(log.lock().unwrap().seeks, vec![expected_signal]);
}

#[kithara::test]
fn owned_hooks_fire_exactly_once_per_chunk(trim_silence: Vec<f32>, zero_packet: &'static [u8]) {
    let log = Arc::new(Mutex::new(CallLog::default()));
    let demuxer = StubDemuxer::with_outcomes(
        zero_packet,
        vec![
            StubOutcome::Frame {
                pts: Duration::from_millis(40),
                duration: Duration::from_millis(20),
            },
            StubOutcome::Frame {
                pts: Duration::from_millis(20),
                duration: Duration::from_millis(20),
            },
            StubOutcome::Frame {
                pts: Duration::ZERO,
                duration: Duration::from_millis(20),
            },
        ],
        Vec::new(),
    );
    let mut decoder = build(&trim_silence, demuxer, Arc::clone(&log));

    for _ in 0..3 {
        let _ = decoder.next_chunk().unwrap();
    }
    let _ = decoder.next_chunk().unwrap();

    // WHY: single-owner Box hooks must fire once per next_chunk — three
    // PCM chunks then one Eof, never doubled, never dropped.
    assert_eq!(
        log.lock().unwrap().chunks,
        vec!["chunk", "chunk", "chunk", "eof"],
    );
}
