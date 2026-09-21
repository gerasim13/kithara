use std::ops::Range;

use kithara_blob::{BlobError, Reader, Writer};
use rangemap::RangeSet;

use super::FrameSpan;

/// Bytes one run occupies in the blob: its start frame and its length.
const RUN_BYTES: usize = 2 * size_of::<u64>();

/// Reading frame coverage out of a byte blob. The framing itself knows nothing
/// about frames, so the run list is read and validated here, where the meaning
/// of a run lives.
pub trait CoverageRead {
    /// Read a length-prefixed run list back into a range set, rejecting an
    /// empty, out-of-order, or overflowing run.
    ///
    /// # Errors
    ///
    /// Errors if the blob ends early or a run is empty, out of order, or
    /// overflowing.
    fn read_coverage(&mut self) -> Result<RangeSet<u64>, BlobError>;
}

/// Writing frame coverage into a byte blob, in the shape [`CoverageRead`]
/// reads back.
pub trait CoverageWrite {
    /// Write a run list the matching [`CoverageRead::read_coverage`] reads
    /// back.
    fn write_coverage(&mut self, coverage: &RangeSet<u64>);
}

impl CoverageRead for Reader<'_> {
    fn read_coverage(&mut self) -> Result<RangeSet<u64>, BlobError> {
        let count = self.read_count(RUN_BYTES)?;
        let mut coverage = RangeSet::new();
        let mut previous_end = None;
        for _ in 0..count {
            let start = self.read_u64()?;
            let frames = self.read_u64()?;
            let run: Range<u64> = start..start.saturating_add(frames);
            if frames == 0 || run.frames() != frames || previous_end.is_some_and(|end| end >= start)
            {
                return Err(BlobError::Corrupt);
            }
            previous_end = Some(run.end);
            coverage.insert(run);
        }
        Ok(coverage)
    }
}

impl CoverageWrite for Writer<'_> {
    fn write_coverage(&mut self, coverage: &RangeSet<u64>) {
        self.write_len(coverage.len());
        for run in coverage.iter() {
            self.write_u64(run.start);
            self.write_u64(run.frames());
        }
    }
}
