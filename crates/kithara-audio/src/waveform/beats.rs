use std::mem::size_of;

use kithara_platform::sync::Arc;
use kithara_warp::GridSegment;

use crate::blob::{self, Blob, BlobError, MAX_PREALLOC, Reader, Writer};

struct Consts;

impl Consts {
    const FRAME_BYTES: usize = size_of::<u64>() + size_of::<u32>() + size_of::<f32>();
    const LEN_PREFIX_BYTES: usize = size_of::<u64>();
    const LIST_COUNT: usize = 3;
    const SEGMENT_BYTES: usize = size_of::<u64>() * 2 + size_of::<f64>();
    const VERSION: u32 = 2;
}

/// One grid marker: its source frame, and the confidence the detector
/// reported for it - `None` where the grid placed it by extrapolation and no
/// detector saw it.
pub(crate) type MarkedBeat = (u64, Option<f32>);

/// Cleaned beat grid for one track. All positions are source frames
/// (decoder/song time, `AudioChunkInfo.frame_offset` space) — never output/stretched
/// time.
#[derive(Debug, Clone, PartialEq, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(get)]
pub struct BeatGrid {
    beats: Arc<[u64]>,
    beat_confidence: Arc<[Option<f32>]>,
    downbeats: Arc<[u64]>,
    downbeat_confidence: Arc<[Option<f32>]>,
    segments: Vec<GridSegment>,
    bpm: f64,
}

impl BeatGrid {
    /// Construct from already-cleaned parts.
    ///
    /// Markers arrive paired with their confidence so the two cannot be
    /// handed over misaligned: the grid never sees two lists to reconcile.
    #[must_use]
    pub fn new(
        bpm: f64,
        beats: Vec<(u64, Option<f32>)>,
        downbeats: Vec<(u64, Option<f32>)>,
        segments: Vec<GridSegment>,
    ) -> Self {
        let beat_confidence = Arc::from_iter(beats.iter().map(|(_, confidence)| *confidence));
        let beats = Arc::from_iter(beats.into_iter().map(|(frame, _)| frame));
        let downbeat_confidence =
            Arc::from_iter(downbeats.iter().map(|(_, confidence)| *confidence));
        let downbeats = Arc::from_iter(downbeats.into_iter().map(|(frame, _)| frame));
        Self {
            beats,
            beat_confidence,
            downbeats,
            downbeat_confidence,
            segments,
            bpm,
        }
    }

    /// Append the versioned grid encoding to caller-owned storage.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        blob::write_to(self, out);
    }
}

impl From<&BeatGrid> for Vec<u8> {
    fn from(grid: &BeatGrid) -> Self {
        blob::to_bytes(grid)
    }
}

impl TryFrom<&[u8]> for BeatGrid {
    type Error = BlobError;

    fn try_from(bytes: &[u8]) -> Result<Self, BlobError> {
        blob::from_bytes(bytes)
    }
}

impl Blob for BeatGrid {
    const VERSION: u32 = Consts::VERSION;

    fn decode(r: &mut Reader<'_>) -> Result<Self, BlobError> {
        let bpm = read_finite(r)?;
        let beats = read_marks(r)?;
        let downbeats = read_marks(r)?;
        let segment_count = r.read_len()?;
        let mut segments: Vec<GridSegment> = Vec::with_capacity(segment_count.min(MAX_PREALLOC));
        for _ in 0..segment_count {
            segments.push(GridSegment::new(
                r.read_u64()?,
                r.read_u64()?,
                read_finite(r)?,
            ));
        }
        Ok(Self::new(bpm, beats, downbeats, segments))
    }

    fn encode(&self, w: &mut Writer<'_>) {
        w.reserve(
            size_of::<f64>()
                + Consts::LIST_COUNT * Consts::LEN_PREFIX_BYTES
                + Consts::FRAME_BYTES * (self.beats.len() + self.downbeats.len())
                + Consts::SEGMENT_BYTES * self.segments.len(),
        );
        w.write_f64(self.bpm);
        write_marks(w, &self.beats, &self.beat_confidence);
        write_marks(w, &self.downbeats, &self.downbeat_confidence);
        w.write_len(self.segments.len());
        for segment in &self.segments {
            w.write_u64(segment.start_frame());
            w.write_u64(segment.end_frame());
            w.write_f64(segment.ratio_correction());
        }
    }
}

fn read_finite(r: &mut Reader<'_>) -> Result<f64, BlobError> {
    let value = r.read_f64()?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(BlobError::Corrupt)
    }
}

#[cfg(test)]
mod bytes_tests {
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;
    use kithara_warp::GridSegment;

    use super::{BeatGrid, BlobError};

    fn sample() -> BeatGrid {
        BeatGrid::new(
            123.5,
            // A mix on purpose: the codec must carry both a detected
            // confidence and the absence of one.
            vec![
                (0, Some(0.9)),
                (22_050, Some(0.5)),
                (44_100, None),
                (66_150, Some(0.75)),
            ],
            vec![(0, Some(0.9)), (88_200, None)],
            vec![
                GridSegment::new(0, 88_200, 1.02),
                GridSegment::new(88_200, 176_400, 0.98),
            ],
        )
    }

    #[kithara::test]
    fn round_trips() {
        let grid = sample();
        let bytes = Vec::<u8>::from(&grid);
        let back = BeatGrid::try_from(bytes.as_slice()).expect("valid blob round-trips");
        assert_eq!(back, grid);
    }

    #[kithara::test]
    fn clone_shares_immutable_marker_storage() {
        let grid = sample();
        let cloned = grid.clone();

        assert!(Arc::ptr_eq(&grid.beats, &cloned.beats));
        assert!(Arc::ptr_eq(&grid.beat_confidence, &cloned.beat_confidence));
        assert!(Arc::ptr_eq(&grid.downbeats, &cloned.downbeats));
        assert!(Arc::ptr_eq(
            &grid.downbeat_confidence,
            &cloned.downbeat_confidence
        ));
    }

    #[kithara::test]
    fn degraded_grid_round_trips() {
        let grid = BeatGrid::new(0.0, Vec::new(), Vec::new(), Vec::new());
        let bytes = Vec::<u8>::from(&grid);
        let back = BeatGrid::try_from(bytes.as_slice()).expect("empty blob round-trips");
        assert_eq!(back, grid);
    }

    #[kithara::test]
    fn rejects_wrong_version() {
        let mut bytes = Vec::<u8>::from(&sample());
        bytes[0] = bytes[0].wrapping_add(1);
        assert!(matches!(
            BeatGrid::try_from(bytes.as_slice()),
            Err(BlobError::Version { .. })
        ));
    }

    #[kithara::test]
    fn rejects_a_confidence_no_detector_could_have_reported() {
        // Header (u32) + bpm (f64) + list length (u64) + first frame (u64)
        // puts the first marker's present flag here.
        let flag = size_of::<u32>() + size_of::<f64>() + size_of::<u64>() + size_of::<u64>();

        let mut bad_flag = Vec::<u8>::from(&sample());
        bad_flag[flag] = 7;
        assert!(
            matches!(
                BeatGrid::try_from(bad_flag.as_slice()),
                Err(BlobError::Corrupt)
            ),
            "a present flag outside yes or no is corruption"
        );

        let mut out_of_range = Vec::<u8>::from(&sample());
        out_of_range[flag + size_of::<u32>()..flag + size_of::<u32>() + size_of::<f32>()]
            .copy_from_slice(&2.0_f32.to_le_bytes());
        assert!(
            matches!(
                BeatGrid::try_from(out_of_range.as_slice()),
                Err(BlobError::Corrupt)
            ),
            "a confidence above one is corruption"
        );
    }

    #[kithara::test]
    fn rejects_corrupt_blobs() {
        let corrupt = |bytes: &[u8]| matches!(BeatGrid::try_from(bytes), Err(BlobError::Corrupt));
        assert!(corrupt(&[0, 0]), "shorter than the version header");

        let mut truncated = Vec::<u8>::from(&sample());
        truncated.pop();
        assert!(corrupt(&truncated), "truncated body");

        let mut trailing = Vec::<u8>::from(&sample());
        trailing.push(0);
        assert!(corrupt(&trailing), "trailing garbage");
    }
}

fn write_marks(w: &mut Writer<'_>, frames: &[u64], confidence: &[Option<f32>]) {
    w.write_len(frames.len());
    for (frame, confidence) in frames.iter().zip(confidence.iter()) {
        w.write_u64(*frame);
        w.write_u32(u32::from(confidence.is_some()));
        w.write_f32(confidence.unwrap_or(0.0));
    }
}

fn read_marks(r: &mut Reader<'_>) -> Result<Vec<MarkedBeat>, BlobError> {
    let count = r.read_len()?;
    let mut out: Vec<MarkedBeat> = Vec::with_capacity(count.min(MAX_PREALLOC));
    for _ in 0..count {
        let frame = r.read_u64()?;
        let present = r.read_u32()?;
        let confidence = r.read_f32()?;
        let confidence = match present {
            0 => None,
            1 => Some(confidence),
            _ => return Err(BlobError::Corrupt),
        };
        if confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return Err(BlobError::Corrupt);
        }
        out.push((frame, confidence));
    }
    Ok(out)
}
