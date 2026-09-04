use std::num::NonZeroU32;

use kithara_audio::{AudioObserveError, AudioObserver};
use kithara_signal::{AudioChunk, AudioSpec};
use num_traits::cast::ToPrimitive;
use tracing::debug;

use super::ring::Writer;
use crate::analyzer::AnalysisToken;

/// The producer side of one analysis pass, named once when the handle is made
/// so offering costs no lookup. A track with no open pass has no handle.
pub struct AnalysisProducer {
    token: AnalysisToken,
    rate: NonZeroU32,
    ring: Writer,
}

impl AnalysisProducer {
    pub(crate) const fn new(ring: Writer, rate: NonZeroU32, token: AnalysisToken) -> Self {
        Self { token, rate, ring }
    }

    /// Offer one interleaved range starting at source frame `at`, downmixed to
    /// mono by the channel mean. Never blocks, allocates, or retains `pcm`.
    ///
    /// # Errors
    /// Returns the same bounded-ingest errors as [`AudioObserver::try_observe`].
    pub fn offer(
        &mut self,
        pcm: &[f32],
        spec: AudioSpec,
        at: u64,
    ) -> Result<(), AudioObserveError> {
        if !self.ring.is_open() {
            return Err(AudioObserveError::Closed);
        }
        if spec.sample_rate != self.rate {
            debug!(
                token = self.token.as_str(),
                axis = self.rate.get(),
                rate = spec.sample_rate.get(),
                "analysis ingest: range measured on another axis; refused"
            );
            return Err(AudioObserveError::UnsupportedSampleRate {
                expected: self.rate,
                actual: spec.sample_rate,
            });
        }

        let channels = usize::from(spec.channels.max(1));
        let frames = pcm.len() / channels;
        let inv = 1.0 / channels.to_f32().unwrap_or(1.0);
        let mono = pcm
            .chunks_exact(channels)
            .map(move |frame| frame.iter().sum::<f32>() * inv);

        self.ring
            .push(at, frames, mono)
            .then_some(())
            .ok_or(AudioObserveError::Full)
    }

    /// The pass this handle feeds.
    #[must_use]
    pub const fn token(&self) -> &AnalysisToken {
        &self.token
    }
}

impl AudioObserver for AnalysisProducer {
    fn try_observe(&mut self, chunk: &AudioChunk) -> Result<(), AudioObserveError> {
        self.offer(&chunk.samples, chunk.spec(), chunk.meta.frame_offset)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::AudioObserveError;
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;

    use super::AnalysisProducer;
    use crate::{producer::ring, test_pools::pools};

    fn spec(rate: u32, channels: u16) -> AudioSpec {
        AudioSpec {
            channels,
            sample_rate: NonZeroU32::new(rate).expect("test rate is non-zero"),
        }
    }

    fn producer(frames: usize, ranges: usize) -> (AnalysisProducer, ring::Reader) {
        let (tx, rx) = ring::open(frames, ranges);
        let rate = NonZeroU32::new(44_100).expect("test rate is non-zero");
        (AnalysisProducer::new(tx, rate, "track-a".into()), rx)
    }

    #[kithara::test]
    fn the_mono_written_is_the_channel_mean() {
        let (mut producer, mut reader) = producer(64, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        // Interleaved stereo: L, R, L, R.
        let pcm = [1.0_f32, 3.0, -2.0, 0.0];
        assert_eq!(
            producer.offer(&pcm, spec(44_100, 2), 512),
            Ok(()),
            "the pass axis matches"
        );

        assert_eq!(reader.pop(&mut out), Some(512));
        assert_eq!(
            &out[..],
            &[2.0, -1.0],
            "each frame is the mean of its channels"
        );
    }

    #[kithara::test]
    fn a_foreign_axis_is_refused_without_writing() {
        let (mut producer, mut reader) = producer(64, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        assert_eq!(
            producer.offer(&[1.0, 1.0], spec(48_000, 2), 0),
            Err(AudioObserveError::UnsupportedSampleRate {
                expected: NonZeroU32::new(44_100).expect("test rate is non-zero"),
                actual: NonZeroU32::new(48_000).expect("test rate is non-zero"),
            })
        );
        assert_eq!(reader.pop(&mut out), None, "nothing reached the transport");
    }

    #[kithara::test]
    fn a_full_transport_reports_the_range_untaken() {
        let (mut producer, _reader) = producer(2, 4);

        assert_eq!(producer.offer(&[1.0, 1.0], spec(44_100, 2), 0), Ok(()));
        assert_eq!(
            producer.offer(&[1.0, 1.0, 1.0, 1.0], spec(44_100, 2), 1),
            Err(AudioObserveError::Full),
            "a range that does not fit is refused whole"
        );
    }

    #[kithara::test]
    fn an_offer_copies_the_source_buffer() {
        let (mut producer, mut reader) = producer(64, 4);
        let pools = pools();

        let mut chunk = pools.get::<f32>();
        chunk.ensure_len(16).expect("the test pool grows to 16");

        assert_eq!(producer.offer(&chunk, spec(44_100, 2), 0), Ok(()));
        drop(chunk);
        let mut out = pools.get::<f32>();
        assert_eq!(
            reader.pop(&mut out),
            Some(0),
            "the range survives its source buffer going back to the pool"
        );
        assert_eq!(
            out.len(),
            8,
            "two channels of sixteen samples is eight frames"
        );
    }

    #[kithara::test]
    fn a_pass_that_ended_refuses_as_closed() {
        let (mut producer, reader) = producer(64, 4);
        assert_eq!(producer.offer(&[1.0, 1.0], spec(44_100, 2), 0), Ok(()));

        drop(reader);
        assert_eq!(
            producer.offer(&[1.0, 1.0], spec(44_100, 2), 1),
            Err(AudioObserveError::Closed),
            "a pass that dropped its half cannot be written to"
        );
    }

    #[kithara::test]
    fn a_mono_source_passes_through_unchanged() {
        let (mut producer, mut reader) = producer(64, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        let pcm = [0.25_f32, -0.5, 0.75];
        assert_eq!(producer.offer(&pcm, spec(44_100, 1), 0), Ok(()));

        assert_eq!(reader.pop(&mut out), Some(0));
        assert_eq!(
            &out[..],
            &pcm[..],
            "the mean of one channel is the channel itself"
        );
    }
}
