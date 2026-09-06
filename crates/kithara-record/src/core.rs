use kithara_encode::{ContainerSession, ContainerWrite, EncoderSession};
use kithara_output::{RenderSink, RenderSinkError};

use crate::{RecordingConfig, RecordingError, RecordingResult, RecordingSink};

/// Shared encode, container, and transaction core for one recording part.
pub struct RecordingCore<S: RecordingSink> {
    container: Option<ContainerSession>,
    encoder: Option<EncoderSession>,
    expected_frames: Option<u64>,
    sink: Option<S>,
    frames: u64,
    channels: usize,
}

impl<S: RecordingSink> RecordingCore<S> {
    /// Open one part and optionally preflight its exact frame count.
    ///
    /// # Errors
    /// Returns an encoding-profile or container-size failure. The supplied
    /// transaction is aborted before returning an error.
    pub fn new(
        config: &RecordingConfig,
        mut sink: S,
        expected_frames: Option<u64>,
    ) -> RecordingResult<Self, S::Error> {
        let encoder = match EncoderSession::new(config.encode()) {
            Ok(encoder) => encoder,
            Err(error) => {
                sink.abort();
                return Err(error.into());
            }
        };
        let container = match ContainerSession::new(config.encode()) {
            Ok(container) => container,
            Err(error) => {
                sink.abort();
                return Err(error.into());
            }
        };
        if let Some(frames) = expected_frames
            && let Err(error) = container.validate_frame_count(frames)
        {
            sink.abort();
            return Err(error.into());
        }

        Ok(Self {
            expected_frames,
            channels: usize::from(config.encode().channels),
            container: Some(container),
            encoder: Some(encoder),
            frames: 0,
            sink: Some(sink),
        })
    }

    fn fail<T>(&mut self, error: RecordingError<S::Error>) -> RecordingResult<T, S::Error> {
        if let Some(sink) = self.sink.as_mut() {
            sink.abort();
        }
        self.sink.take();
        self.encoder.take();
        self.container.take();
        Err(error)
    }

    /// Flush the encoder and container, then atomically publish the part.
    ///
    /// # Errors
    /// Returns a terminal frame-count, encode, sink, or commit failure. No
    /// partial part is published.
    pub fn finish(mut self) -> RecordingResult<S::Output, S::Error> {
        if let Some(expected) = self.expected_frames
            && expected != self.frames
        {
            let actual = self.frames;
            return self.fail(RecordingError::FrameCountMismatch { expected, actual });
        }
        let Some(encoder) = self.encoder.take() else {
            return Err(RecordingError::Inactive);
        };
        let units = match encoder.finish() {
            Ok(units) => units,
            Err(error) => return self.fail(error.into()),
        };
        self.write_units(units)?;
        let Some(container) = self.container.take() else {
            return Err(RecordingError::Inactive);
        };
        let finished = match container.finish() {
            Ok(finished) => finished,
            Err(error) => return self.fail(error.into()),
        };
        self.write_container(finished.writes)?;
        let output = match self.sink.as_mut() {
            Some(sink) => match sink.commit(finished.final_len) {
                Ok(output) => output,
                Err(error) => return self.fail(RecordingError::Sink(error)),
            },
            None => return Err(RecordingError::Inactive),
        };
        self.sink.take();
        Ok(output)
    }

    /// Encode and write complete interleaved PCM frames.
    ///
    /// # Errors
    /// Returns a terminal encode or sink failure and aborts the transaction.
    pub fn push(&mut self, samples: &[f32]) -> RecordingResult<(), S::Error> {
        if self.sink.is_none() {
            return Err(RecordingError::Inactive);
        }
        let Ok(frame_delta) = u64::try_from(samples.len() / self.channels) else {
            return self.fail(RecordingError::FrameCountOverflow);
        };
        let Some(next_frames) = self.frames.checked_add(frame_delta) else {
            return self.fail(RecordingError::FrameCountOverflow);
        };
        let units = match self.encoder.as_mut() {
            Some(encoder) => match encoder.push(samples) {
                Ok(units) => units,
                Err(error) => return self.fail(error.into()),
            },
            None => return Err(RecordingError::Inactive),
        };
        self.write_units(units)?;
        self.frames = next_frames;
        Ok(())
    }

    fn write_container(&mut self, writes: Vec<ContainerWrite>) -> RecordingResult<(), S::Error> {
        for write in writes {
            let result = match self.sink.as_mut() {
                Some(sink) => sink.write_at(write.offset, &write.bytes),
                None => return Err(RecordingError::Inactive),
            };
            if let Err(error) = result {
                return self.fail(RecordingError::Sink(error));
            }
        }
        Ok(())
    }

    fn write_units(
        &mut self,
        units: Vec<kithara_encode::EncodedAccessUnit>,
    ) -> RecordingResult<(), S::Error> {
        for unit in units {
            let writes = match self.container.as_mut() {
                Some(container) => match container.push(unit) {
                    Ok(writes) => writes,
                    Err(error) => return self.fail(error.into()),
                },
                None => return Err(RecordingError::Inactive),
            };
            self.write_container(writes)?;
        }
        Ok(())
    }
}

impl<S: RecordingSink> Drop for RecordingCore<S> {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.as_mut() {
            sink.abort();
        }
    }
}

impl<S: RecordingSink> RenderSink for RecordingCore<S> {
    fn write(&mut self, samples: &[f32]) -> Result<(), RenderSinkError> {
        self.push(samples).map_err(RenderSinkError::new)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use kithara_encode::EncodeConfig;
    use kithara_test_utils::kithara;

    use super::*;

    struct TestSink {
        aborted: Arc<AtomicBool>,
    }

    impl RecordingSink for TestSink {
        type Error = io::Error;
        type Output = ();

        fn abort(&mut self) {
            self.aborted.store(true, Ordering::Release);
        }

        fn commit(&mut self, _final_len: u64) -> Result<Self::Output, Self::Error> {
            Ok(())
        }

        fn write_at(&mut self, _offset: u64, _bytes: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[kithara::test(native, flash(false))]
    fn frame_count_overflow_aborts_transaction() {
        let aborted = Arc::new(AtomicBool::new(false));
        let config = RecordingConfig::builder()
            .encode(
                EncodeConfig::builder()
                    .sample_rate(48_000)
                    .channels(2)
                    .build(),
            )
            .build();
        let mut recording = RecordingCore::new(
            &config,
            TestSink {
                aborted: Arc::clone(&aborted),
            },
            None,
        )
        .expect("open test recording");
        recording.frames = u64::MAX;

        assert!(matches!(
            recording.push(&[0.0, 0.0]),
            Err(RecordingError::FrameCountOverflow)
        ));
        assert!(aborted.load(Ordering::Acquire));
        assert!(matches!(
            recording.push(&[0.0, 0.0]),
            Err(RecordingError::Inactive)
        ));
    }
}
