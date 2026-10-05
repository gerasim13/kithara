use std::io::{self, Cursor, ErrorKind, Read, Seek, SeekFrom};

use kithara_mpa::MpaReader;
use kithara_test_utils::kithara;
use symphonia_core::{
    errors::Error,
    formats::{FormatOptions, FormatReader},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
};

mod consts {
    pub(super) const FRAME_LEN: usize = 417;
    pub(super) const FRAME_COUNT: usize = 4;
    pub(super) const INTERRUPTED_FRAME: usize = 2;
    pub(super) const TRANSIENT_COUNT: usize = 2;
    pub(super) const RESYNC_JUNK_LEN: usize = 128 * 1024 + 17;
    pub(super) const READ_CHUNK: usize = 53;
    pub(super) const ERROR_MESSAGE: &str = "packet input is temporarily unavailable";
}

struct TransientSource {
    cursor: Cursor<Vec<u8>>,
    at: u64,
    kind: ErrorKind,
    remaining: usize,
}

impl Read for TransientSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut count = buffer.len().min(consts::READ_CHUNK);
        if self.remaining > 0 && count > 0 {
            let before_gap = self.at.saturating_sub(self.cursor.position());
            if before_gap == 0 {
                self.remaining -= 1;
                return Err(io::Error::new(self.kind, consts::ERROR_MESSAGE));
            }
            count = count.min(usize::try_from(before_gap).expect("small synthetic source"));
        }
        self.cursor.read(&mut buffer[..count])
    }
}

impl Seek for TransientSource {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "source is forward-only",
        ))
    }
}

impl MediaSource for TransientSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn open(source: TransientSource) -> MpaReader<'static> {
    let stream = MediaSourceStream::new(
        Box::new(source),
        MediaSourceStreamOptions::default(),
    );
    MpaReader::try_new(stream, FormatOptions::default()).expect("valid MPEG headers")
}

fn assert_packet_read_resynchronizes(kind: ErrorKind) {
    let mut bytes = Vec::new();
    for index in 0..consts::FRAME_COUNT {
        if index == consts::INTERRUPTED_FRAME {
            bytes.resize(bytes.len() + consts::RESYNC_JUNK_LEN, 0x55);
        }
        let mut frame =
            vec![u8::try_from(index + 1).expect("four frames") * 0x11; consts::FRAME_LEN];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        bytes.extend_from_slice(&frame);
    }

    for offset in [1, 2, 3, 4, 5, consts::FRAME_LEN / 2, consts::FRAME_LEN - 1] {
        let source = |remaining| TransientSource {
            cursor: Cursor::new(bytes.clone()),
            at: u64::try_from(
                consts::INTERRUPTED_FRAME * consts::FRAME_LEN + consts::RESYNC_JUNK_LEN + offset,
            )
            .expect("small synthetic source"),
            kind,
            remaining,
        };
        let mut expected = open(source(0));
        let mut actual = open(source(consts::TRANSIENT_COUNT));

        for index in 0..consts::FRAME_COUNT {
            if index == consts::INTERRUPTED_FRAME {
                for _ in 0..consts::TRANSIENT_COUNT {
                    match actual.next_packet() {
                        Err(Error::IoError(error)) => {
                            assert_eq!(error.kind(), kind, "gap offset {offset}");
                            assert_eq!(error.to_string(), consts::ERROR_MESSAGE);
                        }
                        other => panic!("expected {kind:?} at offset {offset}, got {other:?}"),
                    }
                }
            }
            let expected = expected
                .next_packet()
                .expect("complete source")
                .expect("packet");
            let actual = actual
                .next_packet()
                .expect("resumed source")
                .expect("packet");
            assert_eq!(actual.track_id, expected.track_id);
            assert_eq!(actual.pts, expected.pts, "gap offset {offset}");
            assert_eq!(actual.dur, expected.dur, "gap offset {offset}");
            assert_eq!(
                actual.data.as_ref(),
                expected.data.as_ref(),
                "gap offset {offset}"
            );
        }
        assert!(expected
            .next_packet()
            .expect("complete end of stream")
            .is_none());
        assert!(actual.next_packet().expect("end of stream").is_none());
    }
}

#[kithara::test(native, flash(false))]
fn packet_read_resynchronizes_after_long_junk_and_would_block() {
    assert_packet_read_resynchronizes(ErrorKind::WouldBlock);
}

#[kithara::test(native, flash(false))]
fn packet_read_resynchronizes_after_long_junk_and_interrupted() {
    assert_packet_read_resynchronizes(ErrorKind::Interrupted);
}
