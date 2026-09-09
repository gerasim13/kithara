use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use kithara_test_fixtures::{SignalAsset, assets::by_name};
use kithara_test_utils::kithara;
use symphonia::core::{
    errors::Error,
    formats::{FormatOptions, FormatReader, probe::Hint},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::MetadataOptions,
};

struct Source {
    cursor: Cursor<Vec<u8>>,
    remaining: Arc<AtomicUsize>,
}

impl Read for Source {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
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

fn reader(remaining: Arc<AtomicUsize>) -> Box<dyn FormatReader> {
    let asset = by_name(SignalAsset::M4A_SINE440_60S.name()).expect("MP4 fixture");
    let source = Source {
        cursor: Cursor::new(asset.bytes().to_vec()),
        remaining,
    };
    let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension("m4a");
    symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("MP4 probe")
}

#[kithara::test]
fn mp4_pooled_packets_preserve_bytes_after_short_buffer_and_interruption() {
    let remaining = Arc::new(AtomicUsize::new(usize::MAX));
    let mut actual = reader(Arc::clone(&remaining));
    let mut expected = reader(Arc::new(AtomicUsize::new(usize::MAX)));
    let capacity = actual
        .packet_buffer_size()
        .expect("packet capacity")
        .expect("borrowed MP4 packets");
    let mut buffer = crate::test_pools::pools().get::<u8>();
    buffer.ensure_len(capacity).expect("pooled packet buffer");
    assert!(actual.read_packet(&mut []).is_err());
    remaining.store(17, Ordering::Relaxed);
    let mut interrupted = false;
    for _ in 0..128 {
        let packet = match actual.read_packet(&mut buffer) {
            Err(Error::IoError(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                interrupted = true;
                actual
                    .read_packet(&mut buffer)
                    .expect("retry interrupted packet")
            }
            result => result.expect("borrowed packet"),
        }
        .expect("MP4 packet");
        let reference = expected
            .next_packet()
            .expect("owned packet")
            .expect("MP4 reference");
        assert_eq!(packet.pts, reference.pts);
        assert_eq!(packet.dur, reference.dur);
        assert_eq!(packet.data, reference.data.as_ref());
    }
    assert!(interrupted, "source interruption must be exercised");
}
