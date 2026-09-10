use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use kithara_stream::PendingReason;
use kithara_test_fixtures::{SignalAsset, assets::by_name};
use kithara_test_utils::kithara;
use symphonia::core::{
    formats::{FormatOptions, FormatReader, probe::Hint},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::MetadataOptions,
};

use crate::{
    demuxer::{DemuxOutcome, Demuxer},
    symphonia::SymphoniaDemuxer,
};

struct Source {
    cursor: Cursor<Vec<u8>>,
    remaining: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
}

impl Read for Source {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let remaining = self.remaining.load(Ordering::Relaxed);
        if remaining == 0 {
            self.remaining.store(usize::MAX, Ordering::Relaxed);
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let len = out.len().min(remaining).min(53);
        let read = self.cursor.read(&mut out[..len])?;
        if remaining != usize::MAX {
            self.remaining.fetch_sub(read, Ordering::Relaxed);
        }
        Ok(read)
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl MediaSource for Source {
    fn byte_len(&self) -> Option<u64> {
        Some(self.cursor.get_ref().len() as u64)
    }

    fn is_seekable(&self) -> bool {
        true
    }
}

fn reader(remaining: Arc<AtomicUsize>, reads: Arc<AtomicUsize>) -> Box<dyn FormatReader> {
    reader_for(SignalAsset::M4A_SINE440_60S, remaining, reads)
}

fn reader_for(
    signal: SignalAsset,
    remaining: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
) -> Box<dyn FormatReader> {
    let asset = by_name(signal.name()).expect("signal fixture");
    let source = Source {
        cursor: Cursor::new(asset.bytes().to_vec()),
        remaining,
        reads,
    };
    let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension(signal.ext());
    symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("signal probe")
}

#[kithara::test]
fn mp4_prepared_frame_consumption_does_not_read_the_source() {
    let reads = Arc::new(AtomicUsize::new(0));
    let format = reader(Arc::new(AtomicUsize::new(usize::MAX)), Arc::clone(&reads));
    let mut demuxer =
        SymphoniaDemuxer::from_reader_with_layout(format, None, None).expect("MP4 demuxer");
    let before = reads.load(Ordering::Relaxed);
    for _ in 0..128 {
        demuxer.prepare_frame().expect("prepare packet");
        let prepared_reads = reads.load(Ordering::Relaxed);
        assert!(matches!(
            demuxer.next_frame_prepared().expect("prepared packet"),
            DemuxOutcome::Frame(_)
        ));
        assert_eq!(reads.load(Ordering::Relaxed), prepared_reads);
        assert!(matches!(
            demuxer.next_frame_prepared().expect("preparation required"),
            DemuxOutcome::Pending(PendingReason::Retry)
        ));
        assert_eq!(reads.load(Ordering::Relaxed), prepared_reads);
    }
    assert!(
        reads.load(Ordering::Relaxed) > before,
        "preparation must exercise source reads"
    );
}

#[kithara::test]
#[case(SignalAsset::WAV_SINE440_60S)]
#[case(SignalAsset::M4A_SINE440_60S)]
fn packet_retry_preserves_bytes_and_timestamps_after_partial_read(#[case] signal: SignalAsset) {
    let remaining = Arc::new(AtomicUsize::new(usize::MAX));
    let mut actual = SymphoniaDemuxer::from_reader_with_layout(
        reader_for(
            signal,
            Arc::clone(&remaining),
            Arc::new(AtomicUsize::new(0)),
        ),
        None,
        None,
    )
    .expect("actual demuxer");
    let mut expected = SymphoniaDemuxer::from_reader_with_layout(
        reader_for(
            signal,
            Arc::new(AtomicUsize::new(usize::MAX)),
            Arc::new(AtomicUsize::new(0)),
        ),
        None,
        None,
    )
    .expect("reference demuxer");
    remaining.store(17, Ordering::Relaxed);
    let mut interrupted = false;
    let mut completed = 0;
    for _ in 0..129 {
        let packet = match actual.next_frame().expect("packet read") {
            DemuxOutcome::Pending(_) => {
                interrupted = true;
                continue;
            }
            DemuxOutcome::Frame(frame) => frame,
            DemuxOutcome::Eof => panic!("unexpected EOF"),
        };
        let DemuxOutcome::Frame(reference) = expected.next_frame().expect("reference read") else {
            panic!("expected reference packet");
        };
        assert_eq!(packet.pts, reference.pts);
        assert_eq!(packet.duration, reference.duration);
        assert_eq!(packet.data, reference.data);
        completed += 1;
        if completed == 128 {
            break;
        }
    }
    assert!(interrupted, "must interrupt a source read");
    assert_eq!(completed, 128);
}
