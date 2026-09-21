use std::{
    io,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_test_utils::kithara;

use super::Fmp4Layout;
use crate::ReadAt;

/// Media timescale of the synthetic track, in ticks per second.
const TIMESCALE: u32 = 44_100;
/// Ticks per synthetic sample, matching an AAC access unit.
const SAMPLE_TICKS: u32 = 1024;
/// Samples per fragment: one fragment is just under a second of audio.
const SAMPLES_PER_FRAGMENT: u32 = 43;
/// Payload bytes per fragment. Large enough that a whole-file read is
/// unmistakable against the header-walk budget below.
const MDAT_BYTES: usize = 1024 * 1024;
const FRAGMENTS: u32 = 8;
/// Bytes the header walk may pull. Box headers are 8 bytes and the
/// `moov`/`moof` boxes are hundreds, so the read-ahead window dominates.
const WALK_BUDGET_BYTES: u64 = 256 * 1024;

fn mp4_box(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = u32::try_from(payload.len() + 8).expect("test box fits u32");
    let mut bytes = Vec::with_capacity(payload.len() + 8);
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(payload);
    bytes
}

fn full_box(name: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![version];
    body.extend_from_slice(&flags.to_be_bytes()[1..]);
    body.extend_from_slice(payload);
    mp4_box(name, &body)
}

fn matrix() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(36);
    for value in [0x0001_0000_i32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes
}

fn init_segment() -> Vec<u8> {
    let mut mvhd = Vec::new();
    mvhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
    mvhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
    mvhd.extend_from_slice(&TIMESCALE.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration: fragmented
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    mvhd.extend_from_slice(&[0u8; 10]); // reserved
    mvhd.extend_from_slice(&matrix());
    mvhd.extend_from_slice(&[0u8; 24]); // pre-defined
    mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id

    let mut tkhd = Vec::new();
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
    tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
    tkhd.extend_from_slice(&[0u8; 4]); // reserved
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // duration
    tkhd.extend_from_slice(&[0u8; 8]); // reserved
    tkhd.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd.extend_from_slice(&0u16.to_be_bytes()); // alternate group
    tkhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    tkhd.extend_from_slice(&[0u8; 2]); // reserved
    tkhd.extend_from_slice(&matrix());
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // width
    tkhd.extend_from_slice(&0u32.to_be_bytes()); // height

    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
    mdhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
    mdhd.extend_from_slice(&TIMESCALE.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes()); // duration
    mdhd.extend_from_slice(&0x55c4u16.to_be_bytes()); // language "und"
    mdhd.extend_from_slice(&[0u8; 2]); // pre-defined

    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(&[0u8; 4]); // pre-defined
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 12]); // reserved
    hdlr.push(0); // empty name

    let dref = full_box(b"dref", 0, 0, &0u32.to_be_bytes());
    let dinf = mp4_box(b"dinf", &dref);

    // An unknown sample entry: `audio_track_timescale` takes the track's
    // `mdhd` timescale, so the codec box itself never has to be decodable.
    let mut stsd = Vec::new();
    stsd.extend_from_slice(&1u32.to_be_bytes()); // entry count
    stsd.extend_from_slice(&mp4_box(b"kthx", &[]));
    let stsd = full_box(b"stsd", 0, 0, &stsd);
    let stts = full_box(b"stts", 0, 0, &0u32.to_be_bytes());
    let stsc = full_box(b"stsc", 0, 0, &0u32.to_be_bytes());
    let mut stsz = Vec::new();
    stsz.extend_from_slice(&0u32.to_be_bytes()); // uniform sample size
    stsz.extend_from_slice(&0u32.to_be_bytes()); // sample count
    let stsz = full_box(b"stsz", 0, 0, &stsz);
    let stco = full_box(b"stco", 0, 0, &0u32.to_be_bytes());

    let mut stbl = stsd;
    stbl.extend_from_slice(&stts);
    stbl.extend_from_slice(&stsc);
    stbl.extend_from_slice(&stsz);
    stbl.extend_from_slice(&stco);
    let stbl = mp4_box(b"stbl", &stbl);

    let mut minf = dinf;
    minf.extend_from_slice(&stbl);
    let minf = mp4_box(b"minf", &minf);

    let mut mdia = full_box(b"mdhd", 0, 0, &mdhd);
    mdia.extend_from_slice(&full_box(b"hdlr", 0, 0, &hdlr));
    mdia.extend_from_slice(&minf);
    let mdia = mp4_box(b"mdia", &mdia);

    let mut trak = full_box(b"tkhd", 0, 3, &tkhd);
    trak.extend_from_slice(&mdia);
    let trak = mp4_box(b"trak", &trak);

    let mut moov = full_box(b"mvhd", 0, 0, &mvhd);
    moov.extend_from_slice(&trak);
    let moov = mp4_box(b"moov", &moov);

    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(b"iso5");
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    ftyp.extend_from_slice(b"dash");
    let mut bytes = mp4_box(b"ftyp", &ftyp);
    bytes.extend_from_slice(&moov);
    bytes
}

fn media_segment(index: u32) -> Vec<u8> {
    let mfhd = full_box(b"mfhd", 0, 0, &(index + 1).to_be_bytes());

    let mut tfhd = Vec::new();
    tfhd.extend_from_slice(&1u32.to_be_bytes()); // track id
    tfhd.extend_from_slice(&SAMPLE_TICKS.to_be_bytes());
    // 0x08: default-sample-duration-present.
    let tfhd = full_box(b"tfhd", 0, 0x08, &tfhd);

    let decode_time = u64::from(index) * u64::from(SAMPLES_PER_FRAGMENT) * u64::from(SAMPLE_TICKS);
    let tfdt = full_box(b"tfdt", 1, 0, &decode_time.to_be_bytes());
    let trun = full_box(b"trun", 0, 0, &SAMPLES_PER_FRAGMENT.to_be_bytes());

    let mut traf = tfhd;
    traf.extend_from_slice(&tfdt);
    traf.extend_from_slice(&trun);
    let traf = mp4_box(b"traf", &traf);

    let mut moof = mfhd;
    moof.extend_from_slice(&traf);
    let mut bytes = mp4_box(b"moof", &moof);
    bytes.extend_from_slice(&mp4_box(b"mdat", &vec![index_fill(index); MDAT_BYTES]));
    bytes
}

fn index_fill(index: u32) -> u8 {
    u8::try_from(index % 251).unwrap_or(0)
}

/// Fragmented-mp4 bytes plus the offset at which the first `moof` starts.
fn fragmented_mp4() -> (Vec<u8>, u64) {
    let mut bytes = init_segment();
    let first_moof = u64::try_from(bytes.len()).expect("test init fits u64");
    for index in 0..FRAGMENTS {
        bytes.extend_from_slice(&media_segment(index));
    }
    (bytes, first_moof)
}

/// Byte source that counts every byte the index walk pulls out of it.
struct CountingSource {
    bytes: Vec<u8>,
    delivered: AtomicU64,
}

impl ReadAt for CountingSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset).map_err(io::Error::other)?;
        let Some(tail) = self.bytes.get(start..) else {
            return Ok(0);
        };
        let n = tail.len().min(buf.len());
        buf[..n].copy_from_slice(&tail[..n]);
        self.delivered.fetch_add(
            u64::try_from(n).map_err(io::Error::other)?,
            Ordering::Relaxed,
        );
        Ok(n)
    }
}

#[kithara::test]
fn layout_walks_headers_without_reading_the_payload() {
    let (bytes, first_moof) = fragmented_mp4();
    let total = u64::try_from(bytes.len()).expect("test file fits u64");
    let source = CountingSource {
        bytes,
        delivered: AtomicU64::new(0),
    };

    let layout = Fmp4Layout::read(&source, total).expect("fragmented mp4 layout");

    let delivered = source.delivered.load(Ordering::Relaxed);
    assert!(
        delivered < WALK_BUDGET_BYTES,
        "layout walk pulled {delivered} bytes from a {total}-byte file; \
         the mdat payload must be seeked over, not read"
    );
    assert_eq!(layout.init_range(), 0..first_moof);
    assert_eq!(layout.timescale(), TIMESCALE);
    assert_eq!(
        u32::try_from(layout.fragments().len()).expect("fragment count fits u32"),
        FRAGMENTS
    );
}

#[kithara::test]
fn fragments_carry_contiguous_ticks_and_byte_ranges() {
    let (bytes, first_moof) = fragmented_mp4();
    let total = u64::try_from(bytes.len()).expect("test file fits u64");
    let source = CountingSource {
        bytes,
        delivered: AtomicU64::new(0),
    };

    let layout = Fmp4Layout::read(&source, total).expect("fragmented mp4 layout");

    let fragment_ticks = u64::from(SAMPLES_PER_FRAGMENT) * u64::from(SAMPLE_TICKS);
    let mut expected_start = first_moof;
    for (idx, fragment) in layout.fragments().iter().enumerate() {
        let index = u64::try_from(idx).expect("fragment index fits u64");
        assert_eq!(fragment.decode_ticks, index * fragment_ticks);
        assert_eq!(fragment.duration_ticks, fragment_ticks);
        assert_eq!(fragment.byte_range.start, expected_start);
        expected_start = fragment.byte_range.end;
    }
    assert_eq!(expected_start, total, "fragments must cover the whole tail");
}
