use std::io::{self, Cursor, ErrorKind, Read, Seek, SeekFrom};

use kithara_mpa::MpaReader;
use kithara_test_utils::kithara;
use symphonia_core::{
    errors::Error,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    units::Timestamp,
};

mod consts {
    /// Sync, MPEG-1, Layer III, no CRC.
    pub(super) const HEADER_LEAD: [u8; 2] = [0xff, 0xfb];
    /// Frames the stream holds: 26.1 s at 44.1 kHz.
    pub(super) const FRAMES: usize = 1_000;
    /// Frames the duration estimate inspects before extrapolating.
    pub(super) const ESTIMATE_WINDOW: usize = 17;
    pub(super) const FRAME_DUR: i64 = 1_152;
    pub(super) const RATE: i64 = 44_100;
    pub(super) const SEEK_SECS: i64 = 10;
    pub(super) const SEEK_FRAME: usize = 300;
    pub(super) const REFERENCE_FRAMES: usize = 2;
    /// Requires two 128 kbps predecessor frames to fill the Layer III reservoir.
    pub(super) const MAIN_DATA_BEGIN: u16 = 500;
    /// Stop after the header and the first byte of the reservoir offset.
    pub(super) const GAP_IN_SIDE_INFO: usize = 5;
    /// Layer III bitrates in kbps, by header index.
    pub(super) const KBPS: [usize; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    pub(super) const KBPS_32: u8 = 0x1;
    pub(super) const KBPS_128: u8 = 0x9;
    pub(super) const KBPS_256: u8 = 0xd;
    pub(super) const KBPS_320: u8 = 0xe;
}

/// One silent MPEG-1 Layer III stereo frame at 44.1 kHz.
fn frame(bitrate_index: u8, padded: bool) -> Vec<u8> {
    let len = 144 * consts::KBPS[usize::from(bitrate_index)] * 1_000 / 44_100 + usize::from(padded);
    let mut frame = vec![0; len];
    frame[..2].copy_from_slice(&consts::HEADER_LEAD);
    frame[2] = (bitrate_index << 4) | (u8::from(padded) << 1);
    frame
}

fn open(source: impl MediaSource + 'static) -> MpaReader<'static> {
    let stream = MediaSourceStream::new(
        Box::new(source),
        MediaSourceStreamOptions::default(),
    );
    match MpaReader::try_new(stream, FormatOptions::default()) {
        Ok(reader) => reader,
        Err(error) => panic!("synthetic MPEG stream must open: {error}"),
    }
}

/// A complete source whose one transient read splits the seek target's side info.
struct SeekGap {
    cursor: Cursor<Vec<u8>>,
    at: u64,
    kind: ErrorKind,
    emitted: bool,
}

impl Read for SeekGap {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.emitted && !buffer.is_empty() {
            let remaining = self.at.saturating_sub(self.cursor.position());
            if remaining == 0 {
                self.emitted = true;
                return Err(io::Error::from(self.kind));
            }
            let count = buffer
                .len()
                .min(usize::try_from(remaining).expect("gap distance fits usize"));
            return self.cursor.read(&mut buffer[..count]);
        }
        self.cursor.read(buffer)
    }
}

impl Seek for SeekGap {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(to)
    }
}

impl MediaSource for SeekGap {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(u64::try_from(self.cursor.get_ref().len()).expect("synthetic MPEG length fits u64"))
    }
}

/// The reader must hand out the leading frame first: estimating the duration
/// reads ahead and has to rewind whatever it decided.
fn assert_starts_at_the_first_frame(reader: &mut MpaReader<'_>, first: &[u8]) {
    let packet = match reader.next_packet() {
        Ok(Some(packet)) => packet,
        Ok(None) => panic!("synthetic MPEG stream ended before its first frame"),
        Err(error) => panic!("synthetic MPEG packet failed: {error}"),
    };
    assert_eq!(packet.pts.get(), 0);
    assert_eq!(packet.data.as_ref(), first);
}

/// Seeks past a point every stream here holds and reads the frame it lands on.
fn assert_seeks_to_ten_seconds(reader: &mut MpaReader<'_>) {
    let required = consts::SEEK_SECS * consts::RATE;
    let seek = reader.seek(
        SeekMode::Accurate,
        SeekTo::Timestamp {
            ts: Timestamp::new(required),
            track_id: 0,
        },
    );
    let seeked = match seek {
        Ok(seeked) => seeked,
        Err(Error::SeekError(kind)) => {
            panic!(
                "a {}s seek inside the stream was refused: {kind:?}",
                consts::SEEK_SECS
            )
        }
        Err(error) => panic!("synthetic MPEG seek failed: {error}"),
    };
    assert_eq!(
        seeked.actual_ts.get(),
        required / consts::FRAME_DUR * consts::FRAME_DUR,
        "the seek lands on the frame holding the target"
    );
    match reader.next_packet() {
        Ok(Some(packet)) => assert_eq!(packet.pts, seeked.actual_ts),
        Ok(None) => panic!("the stream ended where the seek landed"),
        Err(error) => panic!("synthetic MPEG packet failed after the seek: {error}"),
    }
}

/// Without a Xing/Info or VBRI frame the length can only be extrapolated from
/// the frames read at open, and that holds only for a constant bitrate. Here
/// the inspected frames alternate 320/256 kbps and the remaining 983 carry
/// 32 kbps: averaging them puts the end near 3.3 s instead of 26.1 s, and a
/// published end that early refuses a seek to 10 s the stream holds.
#[kithara::test]
fn a_headerless_stream_with_mixed_leading_bitrates_publishes_no_duration() {
    let frames: Vec<Vec<u8>> = (0..consts::FRAMES)
        .map(|index| match index {
            i if i >= consts::ESTIMATE_WINDOW => frame(consts::KBPS_32, false),
            i if i % 2 == 0 => frame(consts::KBPS_320, false),
            _ => frame(consts::KBPS_256, false),
        })
        .collect();
    let mut reader = open(Cursor::new(frames.concat()));

    assert_eq!(
        reader.tracks()[0].num_frames,
        None,
        "differing bitrates leave the length unknown rather than extrapolated"
    );
    assert_starts_at_the_first_frame(&mut reader, &frames[0]);
    assert_seeks_to_ten_seconds(&mut reader);
}

/// A constant bitrate still yields a duration without any tag, even though
/// slot padding makes consecutive frames differ in length.
#[kithara::test]
fn a_headerless_constant_bitrate_stream_keeps_its_estimated_duration() {
    let frames: Vec<Vec<u8>> = (0..consts::FRAMES)
        .map(|index| frame(consts::KBPS_128, index % 2 == 1))
        .collect();
    let mut reader = open(Cursor::new(frames.concat()));

    let expected = u64::try_from(consts::FRAMES).expect("frame count fits u64")
        * u64::try_from(consts::FRAME_DUR).expect("frame duration fits u64");
    assert_eq!(reader.tracks()[0].num_frames, Some(expected));
    assert_starts_at_the_first_frame(&mut reader, &frames[0]);
    assert_seeks_to_ten_seconds(&mut reader);
}

#[kithara::test]
fn an_accurate_seek_resumes_across_transient_side_info_reads() {
    let mut frames: Vec<_> = (0..consts::FRAMES)
        .map(|_| frame(consts::KBPS_128, false))
        .collect();
    frames[consts::SEEK_FRAME][4] =
        u8::try_from(consts::MAIN_DATA_BEGIN >> 1).expect("the reservoir offset's high byte");
    frames[consts::SEEK_FRAME][5] =
        u8::try_from((consts::MAIN_DATA_BEGIN & 1) << 7).expect("the reservoir offset's low bit");
    let bytes = frames.concat();
    let target = || SeekTo::Timestamp {
        ts: Timestamp::new(
            i64::try_from(consts::SEEK_FRAME).expect("frame index fits i64") * consts::FRAME_DUR
                + consts::FRAME_DUR / 2,
        ),
        track_id: 0,
    };
    let gap_at = frames[..consts::SEEK_FRAME]
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        + consts::GAP_IN_SIDE_INFO;

    for kind in [ErrorKind::WouldBlock, ErrorKind::Interrupted] {
        let mut uninterrupted = open(Cursor::new(bytes.clone()));
        let expected = uninterrupted
            .seek(SeekMode::Accurate, target())
            .expect("uninterrupted seek reaches its reference frame");
        assert_eq!(
            expected.actual_ts.get(),
            i64::try_from(consts::SEEK_FRAME - consts::REFERENCE_FRAMES)
                .expect("frame index fits i64")
                * consts::FRAME_DUR,
            "the target needs earlier frames for its reservoir"
        );
        let mut resumed = open(SeekGap {
            cursor: Cursor::new(bytes.clone()),
            at: u64::try_from(gap_at).expect("gap position fits u64"),
            kind,
            emitted: false,
        });
        let pending = resumed.seek(SeekMode::Accurate, target());
        assert!(matches!(pending, Err(Error::IoError(error)) if error.kind() == kind));
        let actual = resumed
            .seek(SeekMode::Accurate, target())
            .expect("the same seek resumes once its input is available");
        assert_eq!(actual.required_ts, expected.required_ts);
        assert_eq!(actual.actual_ts, expected.actual_ts, "after {kind:?}");
        for _ in 0..=consts::REFERENCE_FRAMES {
            let expected = uninterrupted
                .next_packet()
                .expect("uninterrupted packet")
                .expect("reference and target packets exist");
            let actual = resumed
                .next_packet()
                .expect("resumed packet")
                .expect("reference and target packets exist");
            assert_eq!(actual.pts, expected.pts, "after {kind:?}");
            assert_eq!(actual.dur, expected.dur, "after {kind:?}");
            assert_eq!(actual.data.as_ref(), expected.data.as_ref(), "after {kind:?}");
        }
    }
}
