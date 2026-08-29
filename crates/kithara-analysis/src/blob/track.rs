use std::num::NonZeroU32;

use crate::{
    AnalysisFingerprint, BeatGrid, BeatSnapshot, Coverage, FrameRange, GridState, TrackAnalysis,
    Waveform,
    blob::{BlobError, Reader, Writer},
};

const TRACK_ANALYSIS_BYTES_VERSION: u32 = 0x4b41_0005;

impl TrackAnalysis {
    /// Append this snapshot to caller-owned storage using the durable analysis format.
    ///
    /// # Errors
    ///
    /// Returns [`BlobError::TooLarge`] when a length does not fit the format.
    pub fn write_to(&self, out: &mut Vec<u8>) -> Result<(), BlobError> {
        let mut writer = Writer::new(out);
        writer.write_u32(TRACK_ANALYSIS_BYTES_VERSION);
        writer.write_str(self.token().as_str())?;
        writer.write_u32(self.source_sample_rate().get());
        writer.write_optional_u64(self.extent());
        writer.write_u64(self.revision());
        write_ranges(&mut writer, self.coverage().runs())?;

        writer.write_str(self.fingerprint().waveform().unwrap_or_default())?;
        writer.write_section(|out| {
            if let Some(waveform) = self.waveform() {
                waveform.write_to(out);
            }
        })?;

        writer.write_str(self.fingerprint().beat().unwrap_or_default())?;
        let beat = self.beat();
        writer.write_section(|out| {
            if let Some(beat) = beat {
                beat.grid().write_to(out);
            }
        })?;
        writer.write_bool(beat.is_some_and(|beat| beat.state() == GridState::Final));
        write_ranges(&mut writer, beat.map_or(&[], BeatSnapshot::unanalysed))?;
        Ok(())
    }
}

impl TryFrom<(&[u8], &AnalysisFingerprint)> for TrackAnalysis {
    type Error = BlobError;

    fn try_from((bytes, active): (&[u8], &AnalysisFingerprint)) -> Result<Self, Self::Error> {
        let mut reader = Reader::new(bytes);
        let version = reader.read_u32()?;
        if version != TRACK_ANALYSIS_BYTES_VERSION {
            return Err(BlobError::Version {
                found: version,
                expected: TRACK_ANALYSIS_BYTES_VERSION,
            });
        }

        let token = reader.read_str()?;
        let source_sample_rate = NonZeroU32::new(reader.read_u32()?).ok_or(BlobError::Corrupt)?;
        let extent = reader.read_optional_u64()?;
        let revision = reader.read_u64()?;
        let coverage = read_ranges(&mut reader)?;

        let waveform_tag = reader.read_str()?;
        let waveform_bytes = reader.read_section()?;
        let beat_tag = reader.read_str()?;
        let grid_bytes = reader.read_section()?;
        let final_grid = reader.read_bool()?;
        let unanalysed = read_ranges(&mut reader)?;
        reader.finish()?;

        let waveform_ok = waveform_tag == active.waveform().unwrap_or_default();
        let beat_ok = beat_tag == active.beat().unwrap_or_default();
        if !waveform_ok && !beat_ok {
            return Err(BlobError::Fingerprint);
        }

        let waveform = (waveform_ok && !waveform_bytes.is_empty())
            .then(|| Waveform::try_from(waveform_bytes))
            .transpose()
            .map_err(|_| BlobError::Corrupt)?;
        let grid = (beat_ok && !grid_bytes.is_empty())
            .then(|| BeatGrid::try_from(grid_bytes))
            .transpose()
            .map_err(|_| BlobError::Corrupt)?;
        let state = if final_grid {
            GridState::Final
        } else {
            GridState::Provisional
        };

        let mut restored = Coverage::default();
        for range in coverage {
            restored.insert(range);
        }

        Ok(Self::builder()
            .token(token.as_str().into())
            .revision(revision)
            .source_sample_rate(source_sample_rate)
            .maybe_extent(extent)
            .settled(true)
            .coverage(restored)
            .fingerprint(AnalysisFingerprint::new(
                beat_ok.then_some(beat_tag.as_str()),
                waveform_ok.then_some(waveform_tag.as_str()),
            ))
            .maybe_waveform(waveform)
            .maybe_beat(grid.map(|grid| BeatSnapshot::new(grid, state, unanalysed)))
            .build())
    }
}

fn write_ranges(writer: &mut Writer<'_>, ranges: &[FrameRange]) -> Result<(), BlobError> {
    let count = u32::try_from(ranges.len()).map_err(|_| BlobError::TooLarge)?;
    writer.write_u32(count);
    for range in ranges {
        writer.write_u64(range.start());
        writer.write_u64(range.frames());
    }
    Ok(())
}

fn read_ranges(reader: &mut Reader<'_>) -> Result<Vec<FrameRange>, BlobError> {
    let count = usize::try_from(reader.read_u32()?).map_err(|_| BlobError::Corrupt)?;
    if count.saturating_mul(16) > reader.remaining() {
        return Err(BlobError::Corrupt);
    }
    let mut ranges: Vec<FrameRange> = Vec::with_capacity(count);
    for _ in 0..count {
        let start = reader.read_u64()?;
        let frames = reader.read_u64()?;
        ranges.push(FrameRange::new(start, frames));
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_bufpool::{ByteBuffer, BytePool};
    use kithara_test_utils::kithara;
    use kithara_warp::GridSegment;

    use super::*;

    struct Consts;

    impl Consts {
        const BEAT_TAG: &'static str = "beat:test:v1";
        const TOKEN: &'static str = "assets/track.analysis";
        const V5_FIXTURE: &'static [u8] = &[
            0x05, 0x00, 0x41, 0x4b, 0x09, 0x00, 0x00, 0x00, 0x67, 0x6f, 0x6c, 0x64, 0x65, 0x6e,
            0x2d, 0x76, 0x35, 0x80, 0xbb, 0x00, 0x00, 0x01, 0xd2, 0x04, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xc8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x77, 0x61, 0x76, 0x65, 0x3a, 0x76,
            0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x62,
            0x65, 0x61, 0x74, 0x3a, 0x76, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00,
        ];
    }

    fn fingerprint(wave: &str, beat: &str) -> AnalysisFingerprint {
        AnalysisFingerprint::new(Some(beat), Some(wave))
    }

    fn active() -> AnalysisFingerprint {
        fingerprint("wave:native:max1500:v1", Consts::BEAT_TAG)
    }

    fn rate() -> NonZeroU32 {
        NonZeroU32::new(44_100).expect("fixture rate is non-zero")
    }

    fn wave() -> Waveform {
        Waveform::try_from([1, 0, 0, 0, 0, 0, 0, 63, 0, 0, 0, 63, 0, 0, 0, 63].as_slice())
            .expect("hand-built blob is valid")
    }

    fn grid() -> BeatGrid {
        BeatGrid::new(
            128.0,
            vec![(0, Some(0.9)), (10_000, Some(0.75)), (20_000, None)],
            vec![(0, Some(0.9)), (40_000, None)],
            vec![GridSegment::new(0, 40_000, 1.01)],
        )
    }

    fn analysis(beat: Option<BeatGrid>, waveform: Option<Waveform>, extent: u64) -> TrackAnalysis {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, extent));
        TrackAnalysis::builder()
            .token(Consts::TOKEN.into())
            .revision(7)
            .source_sample_rate(rate())
            .extent(extent)
            .settled(true)
            .coverage(coverage)
            .fingerprint(active())
            .maybe_waveform(waveform)
            .maybe_beat(beat.map(|grid| {
                BeatSnapshot::new(grid, GridState::Provisional, vec![FrameRange::new(100, 50)])
            }))
            .build()
    }

    fn encode(analysis: &TrackAnalysis) -> ByteBuffer {
        let mut bytes = BytePool::default().get();
        analysis.write_to(&mut bytes).expect("encodes");
        bytes
    }

    #[kithara::test]
    fn frozen_v5_fixture_decodes_and_reencodes_identically() {
        let active = fingerprint("wave:v1", "beat:v1");
        let decoded = TrackAnalysis::try_from((Consts::V5_FIXTURE, &active)).expect("v5 decodes");

        assert_eq!(decoded.token().as_str(), "golden-v5");
        assert_eq!(decoded.source_sample_rate().get(), 48_000);
        assert_eq!(decoded.extent(), Some(1_234));
        assert_eq!(decoded.revision(), 9);
        assert_eq!(
            decoded.coverage().runs(),
            &[FrameRange::new(0, 100), FrameRange::new(200, 50)]
        );
        assert_eq!(decoded.fingerprint(), &active);
        assert!(decoded.waveform().is_none());
        assert!(decoded.beat().is_none());

        let mut encoded = BytePool::default().get();
        decoded.write_to(&mut encoded).expect("v5 re-encodes");
        assert_eq!(encoded.as_slice(), Consts::V5_FIXTURE);
    }

    #[kithara::test]
    fn codec_round_trips_waveform_and_beat() {
        let bytes = encode(&analysis(Some(grid()), Some(wave()), 1_234_567));
        let back = TrackAnalysis::try_from((&bytes[..], &active())).expect("decodes");
        assert_eq!(
            back.waveform().expect("waveform survives").buckets(),
            wave().buckets()
        );
        assert_eq!(back.beat().expect("beat grid survives").grid(), &grid());
        assert_eq!(back.source_frames(), 1_234_567);
    }

    #[kithara::test]
    fn codec_round_trips_without_beat() {
        let bytes = encode(&analysis(None, Some(wave()), 0));
        let back = TrackAnalysis::try_from((&bytes[..], &active())).expect("decodes");
        assert!(back.waveform().is_some());
        assert!(back.beat().is_none());
    }

    #[kithara::test]
    fn codec_round_trips_beat_only() {
        let bytes = encode(&analysis(Some(grid()), None, 0));
        let back = TrackAnalysis::try_from((&bytes[..], &active())).expect("decodes");
        assert!(back.waveform().is_none());
        assert_eq!(back.beat().expect("beat grid survives").grid(), &grid());
    }

    #[kithara::test]
    fn stale_fingerprint_is_a_miss() {
        assert!(matches!(
            TrackAnalysis::try_from((
                &encode(&analysis(Some(grid()), Some(wave()), 1))[..],
                &fingerprint("other-wave", "other-beat"),
            )),
            Err(BlobError::Fingerprint)
        ));
    }

    #[kithara::test]
    fn every_snapshot_field_round_trips() {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 400));
        coverage.insert(FrameRange::new(600, 400));
        let want = TrackAnalysis::builder()
            .token(Consts::TOKEN.into())
            .revision(11)
            .source_sample_rate(rate())
            .extent(1_000)
            .coverage(coverage)
            .fingerprint(active())
            .waveform(wave())
            .beat(BeatSnapshot::new(
                grid(),
                GridState::Provisional,
                vec![FrameRange::new(400, 200)],
            ))
            .build();
        let bytes = encode(&want);
        let got = TrackAnalysis::try_from((&bytes[..], &active())).expect("decodes");

        assert_eq!(got.token(), want.token());
        assert_eq!(got.source_sample_rate(), want.source_sample_rate());
        assert_eq!(got.extent(), want.extent());
        assert_eq!(got.coverage(), want.coverage());
        assert_eq!(got.revision(), want.revision());
        assert_eq!(got.waveform_completeness(), want.waveform_completeness());
        let (got_beat, want_beat) = (
            got.beat().expect("beat survives"),
            want.beat().expect("beat fixture"),
        );
        assert_eq!(got_beat.grid(), want_beat.grid());
        assert_eq!(got_beat.state(), want_beat.state());
        assert_eq!(got_beat.unanalysed(), want_beat.unanalysed());
    }

    #[kithara::test]
    fn rejects_non_boolean_flags() {
        let mut bytes = encode(&analysis(Some(grid()), Some(wave()), 1));
        let extent_flag =
            TRACK_ANALYSIS_BYTES_VERSION.to_le_bytes().len() + 4 + Consts::TOKEN.len() + 4;
        bytes[extent_flag] = 2;
        assert!(matches!(
            TrackAnalysis::try_from((&bytes[..], &active())),
            Err(BlobError::Corrupt)
        ));
    }
}
