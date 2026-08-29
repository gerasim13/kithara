use std::mem::size_of;

use kithara_platform::sync::Arc;

use crate::blob::{self, Blob, BlobError, MAX_PREALLOC, Reader, Writer};

struct Consts;

impl Consts {
    const FRAME_BYTES: usize = size_of::<u64>() + size_of::<u32>() + size_of::<f32>();
    const LEN_PREFIX_BYTES: usize = size_of::<u64>();
    const LIST_COUNT: usize = 3;
    const SEGMENT_BYTES: usize = size_of::<u64>() * 2 + size_of::<f64>();
    const VERSION: u32 = 2;
}

/// One artifact marker: its source frame, and the confidence the detector
/// reported for it - `None` where analysis placed it by extrapolation and no
/// detector saw it.
pub(crate) type MarkedBeat = (u64, Option<f32>);

/// One private tempo-fit region retained by the versioned artifact codec.
///
/// This is an analyzer result, not a `kithara-warp` map or render plan.
#[derive(Debug, Clone, Copy, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, vis = "pub(crate)")]
pub(crate) struct FitRegion {
    start_frame: u64,
    end_frame: u64,
    ratio_correction: f64,
}

impl FitRegion {
    pub(crate) const fn new(start_frame: u64, end_frame: u64, ratio_correction: f64) -> Self {
        Self {
            start_frame,
            end_frame,
            ratio_correction,
        }
    }
}

/// Cleaned beat-analysis artifact for one track.
///
/// All positions use source frames (`AudioChunkInfo.frame_offset` space),
/// never output, host-rate, or stretched time. This value carries no live grid
/// identity or revision; those belong to `kithara-warp::BeatGrid` implementors.
#[derive(Debug, Clone, PartialEq, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(opt_in, get)]
pub struct BeatArtifact {
    #[field(get)]
    beats: Arc<[u64]>,
    #[field(get)]
    beat_confidence: Arc<[Option<f32>]>,
    #[field(get)]
    downbeats: Arc<[u64]>,
    #[field(get)]
    downbeat_confidence: Arc<[Option<f32>]>,
    regions: Vec<FitRegion>,
    #[field(get, copy)]
    bpm: f64,
}

impl BeatArtifact {
    /// Constructs an artifact from already-cleaned detector facts.
    ///
    /// Markers arrive paired with their confidence so the two cannot be
    /// handed over misaligned: consumers never see two lists to reconcile.
    #[must_use]
    pub fn new(
        bpm: f64,
        beats: Vec<(u64, Option<f32>)>,
        downbeats: Vec<(u64, Option<f32>)>,
    ) -> Self {
        Self::with_regions(bpm, beats, downbeats, Vec::new())
    }

    pub(crate) fn with_regions(
        bpm: f64,
        beats: Vec<MarkedBeat>,
        downbeats: Vec<MarkedBeat>,
        regions: Vec<FitRegion>,
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
            regions,
            bpm,
        }
    }

    #[cfg(any(test, all(not(target_arch = "wasm32"), feature = "analysis-beat")))]
    pub(crate) fn regions(&self) -> &[FitRegion] {
        &self.regions
    }

    /// Appends the versioned artifact encoding to caller-owned storage.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        blob::write_to(self, out);
    }
}

impl TryFrom<&[u8]> for BeatArtifact {
    type Error = BlobError;

    fn try_from(bytes: &[u8]) -> Result<Self, BlobError> {
        blob::from_bytes(bytes)
    }
}

impl Blob for BeatArtifact {
    const VERSION: u32 = Consts::VERSION;

    fn decode(r: &mut Reader<'_>) -> Result<Self, BlobError> {
        let bpm = read_finite(r)?;
        let beats = read_marks(r)?;
        let downbeats = read_marks(r)?;
        let region_count = r.read_len()?;
        let mut regions: Vec<FitRegion> = Vec::with_capacity(region_count.min(MAX_PREALLOC));
        for _ in 0..region_count {
            regions.push(FitRegion::new(
                r.read_u64()?,
                r.read_u64()?,
                read_finite(r)?,
            ));
        }
        Ok(Self::with_regions(bpm, beats, downbeats, regions))
    }

    fn encode(&self, w: &mut Writer<'_>) {
        w.reserve(
            size_of::<f64>()
                + Consts::LIST_COUNT * Consts::LEN_PREFIX_BYTES
                + Consts::FRAME_BYTES * (self.beats.len() + self.downbeats.len())
                + Consts::SEGMENT_BYTES * self.regions.len(),
        );
        w.write_f64(self.bpm);
        write_marks(w, &self.beats, &self.beat_confidence);
        write_marks(w, &self.downbeats, &self.downbeat_confidence);
        w.write_len(self.regions.len());
        for region in &self.regions {
            w.write_u64(region.start_frame());
            w.write_u64(region.end_frame());
            w.write_f64(region.ratio_correction());
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

    use super::{BeatArtifact, BlobError, FitRegion};
    use crate::blob::to_bytes;

    const V2_FIXTURE: &[u8] = &[
        0x02, 0x00, 0x00, 0x00, // version
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5e, 0x40, // 120 BPM
        0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // two beats
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // frame 0
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, // observed, confidence 1
        0x22, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // frame 22_050
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // extrapolated
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // one downbeat
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // frame 0
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, // observed, confidence 1
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // one fit region
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // start 0
        0x44, 0xac, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // end 44_100
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, // ratio 1
    ];

    fn sample() -> BeatArtifact {
        BeatArtifact::with_regions(
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
                FitRegion::new(0, 88_200, 1.02),
                FitRegion::new(88_200, 176_400, 0.98),
            ],
        )
    }

    #[kithara::test]
    fn round_trips() {
        let artifact = sample();
        let bytes = to_bytes(&artifact);
        let back = BeatArtifact::try_from(bytes.as_slice()).expect("valid blob round-trips");
        assert_eq!(back, artifact);
    }

    #[kithara::test]
    fn frozen_v2_fixture_decodes_and_reencodes_identically() {
        let artifact = BeatArtifact::try_from(V2_FIXTURE).expect("frozen v2 artifact decodes");

        assert_eq!(artifact.bpm(), 120.0);
        assert_eq!(artifact.beats(), [0, 22_050]);
        assert_eq!(artifact.beat_confidence(), [Some(1.0), None]);
        assert_eq!(artifact.downbeats(), [0]);
        assert_eq!(artifact.downbeat_confidence(), [Some(1.0)]);
        assert_eq!(artifact.regions(), [FitRegion::new(0, 44_100, 1.0)]);

        let mut encoded = Vec::new();
        artifact.write_to(&mut encoded);
        assert_eq!(encoded, V2_FIXTURE);
    }

    #[kithara::test]
    fn clone_shares_immutable_marker_storage() {
        let artifact = sample();
        let cloned = artifact.clone();

        assert!(Arc::ptr_eq(&artifact.beats, &cloned.beats));
        assert!(Arc::ptr_eq(
            &artifact.beat_confidence,
            &cloned.beat_confidence
        ));
        assert!(Arc::ptr_eq(&artifact.downbeats, &cloned.downbeats));
        assert!(Arc::ptr_eq(
            &artifact.downbeat_confidence,
            &cloned.downbeat_confidence
        ));
    }

    #[kithara::test]
    fn degraded_grid_round_trips() {
        let artifact = BeatArtifact::new(0.0, Vec::new(), Vec::new());
        let bytes = to_bytes(&artifact);
        let back = BeatArtifact::try_from(bytes.as_slice()).expect("empty blob round-trips");
        assert_eq!(back, artifact);
    }

    #[kithara::test]
    fn rejects_wrong_version() {
        let mut bytes = to_bytes(&sample());
        bytes[0] = bytes[0].wrapping_add(1);
        assert!(matches!(
            BeatArtifact::try_from(bytes.as_slice()),
            Err(BlobError::Version { .. })
        ));
    }

    #[kithara::test]
    fn rejects_a_confidence_no_detector_could_have_reported() {
        // Header (u32) + bpm (f64) + list length (u64) + first frame (u64)
        // puts the first marker's present flag here.
        let flag = size_of::<u32>() + size_of::<f64>() + size_of::<u64>() + size_of::<u64>();

        let mut bad_flag = to_bytes(&sample());
        bad_flag[flag] = 7;
        assert!(
            matches!(
                BeatArtifact::try_from(bad_flag.as_slice()),
                Err(BlobError::Corrupt)
            ),
            "a present flag outside yes or no is corruption"
        );

        let mut out_of_range = to_bytes(&sample());
        out_of_range[flag + size_of::<u32>()..flag + size_of::<u32>() + size_of::<f32>()]
            .copy_from_slice(&2.0_f32.to_le_bytes());
        assert!(
            matches!(
                BeatArtifact::try_from(out_of_range.as_slice()),
                Err(BlobError::Corrupt)
            ),
            "a confidence above one is corruption"
        );
    }

    #[kithara::test]
    fn rejects_corrupt_blobs() {
        let corrupt =
            |bytes: &[u8]| matches!(BeatArtifact::try_from(bytes), Err(BlobError::Corrupt));
        assert!(corrupt(&[0, 0]), "shorter than the version header");

        let mut truncated = to_bytes(&sample());
        truncated.pop();
        assert!(corrupt(&truncated), "truncated body");

        let mut trailing = to_bytes(&sample());
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
