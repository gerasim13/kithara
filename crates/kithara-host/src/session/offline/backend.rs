use std::num::NonZeroU32;

use bon::Builder;
use firewheel::{
    StreamInfo,
    backend::{AudioBackend, BackendProcessInfo},
    node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_platform::time::{Duration, Instant};

use super::{CHANNELS, OfflineSessionError};

#[derive(Builder, Clone, Copy)]
#[builder(state_mod(vis = "pub(crate)"))]
pub(super) struct BackendConfig {
    pub(super) declared_latency: Duration,
    pub(super) block_frames: NonZeroU32,
    pub(super) declick_frames: NonZeroU32,
    pub(super) sample_rate: NonZeroU32,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self::builder()
            .block_frames(NonZeroU32::MIN)
            .declick_frames(NonZeroU32::MIN)
            .declared_latency(Duration::ZERO)
            .sample_rate(NonZeroU32::MIN)
            .build()
    }
}

pub(super) struct OfflineBackend {
    sample_rate: NonZeroU32,
    processor: Option<FirewheelProcessor<Self>>,
}

impl OfflineBackend {
    pub(super) fn render(
        &mut self,
        position: u64,
        frames: usize,
        output: &mut [f32],
    ) -> Result<(), OfflineSessionError> {
        let processor = self
            .processor
            .as_mut()
            .ok_or(OfflineSessionError::ProcessorUnavailable)?;
        let rate = u64::from(self.sample_rate.get());
        let whole_seconds = position / rate;
        let remainder =
            u32::try_from(position % rate).map_err(|_| OfflineSessionError::TimelineOverflow)?;
        let process_info = BackendProcessInfo {
            frames,
            num_in_channels: 0,
            num_out_channels: CHANNELS,
            process_timestamp: Instant::now(),
            duration_since_stream_start: Duration::from_secs(whole_seconds)
                + Duration::from_secs_f64(f64::from(remainder) / f64::from(self.sample_rate.get())),
            input_stream_status: StreamStatus::empty(),
            output_stream_status: StreamStatus::empty(),
            dropped_frames: 0,
        };
        processor.process_interleaved(&[], output, process_info);
        Ok(())
    }
}

impl AudioBackend for OfflineBackend {
    type Config = BackendConfig;
    type Enumerator = ();
    type Instant = Instant;
    type StartStreamError = OfflineSessionError;
    type StreamError = OfflineSessionError;

    fn delay_from_last_process(&self, _process_timestamp: Self::Instant) -> Option<Duration> {
        None
    }

    fn enumerator() -> Self::Enumerator {}

    fn poll_status(&mut self) -> Result<(), Self::StreamError> {
        Ok(())
    }

    fn set_processor(&mut self, processor: FirewheelProcessor<Self>) {
        self.processor = Some(processor);
    }

    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        let stream = StreamInfo {
            sample_rate: config.sample_rate,
            sample_rate_recip: 1.0 / f64::from(config.sample_rate.get()),
            prev_sample_rate: config.sample_rate,
            max_block_frames: config.block_frames,
            num_stream_in_channels: 0,
            num_stream_out_channels: u32::try_from(CHANNELS)
                .map_err(|_| OfflineSessionError::ChannelCountOverflow)?,
            input_to_output_latency_seconds: config.declared_latency.as_secs_f64(),
            declick_frames: config.declick_frames,
            output_device_id: "offline".to_owned(),
            input_device_id: None,
        };
        Ok((
            Self {
                processor: None,
                sample_rate: config.sample_rate,
            },
            stream,
        ))
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn stream_uses_configured_block_declick_and_latency() {
        let block_frames = NonZeroU32::new(127).expect("fixture block frames");
        let declick_frames = NonZeroU32::new(23).expect("fixture declick frames");
        let declared_latency = Duration::from_millis(7);
        let sample_rate = NonZeroU32::new(48_000).expect("fixture sample rate");

        let config = BackendConfig::builder()
            .block_frames(block_frames)
            .declick_frames(declick_frames)
            .declared_latency(declared_latency)
            .sample_rate(sample_rate)
            .build();
        let (_, stream) = OfflineBackend::start_stream(config).expect("fixture backend stream");

        assert_eq!(stream.max_block_frames, block_frames);
        assert_eq!(stream.declick_frames, declick_frames);
        assert_eq!(
            stream.input_to_output_latency_seconds,
            declared_latency.as_secs_f64()
        );
        assert_eq!(stream.sample_rate, sample_rate);
    }
}
