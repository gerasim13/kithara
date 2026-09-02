use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::{NonZeroU32, NonZeroUsize},
};

use bon::Builder;
use kithara_platform::time::Duration;
use kithara_stream::{AudioCodec, ContainerFormat};
use kithara_worker::Priority;

use crate::{BroadcastError, BroadcastResult};

/// Audio, segmentation, retention, and origin settings for a live broadcast.
#[derive(Debug, Clone, Builder)]
#[non_exhaustive]
pub struct BroadcastConfig {
    /// Media duration a segment is cut at.
    #[builder(default = Duration::from_secs(4))]
    pub segment_target: Duration,
    /// Loopback on an ephemeral port.
    #[builder(default = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))]
    pub bind: SocketAddr,
    /// Channel count of the mix.
    #[builder(default = 2)]
    pub channels: u16,
    /// Sample rate of the mix.
    #[builder(default = 48_000)]
    pub sample_rate: u32,
    /// AAC-LC bit rate the encoder targets.
    #[builder(default = 128_000)]
    pub bit_rate: u64,
    /// Codec emitted into HLS media segments.
    #[builder(default = AudioCodec::AacLc)]
    pub codec: AudioCodec,
    /// Container carried by HLS media segments.
    #[builder(default = ContainerFormat::Adts)]
    pub container: ContainerFormat,
    /// Segments kept fetchable past the playlist window.
    #[builder(default = 3)]
    pub grace: usize,
    /// Segments a client sees in the playlist.
    #[builder(default = 6)]
    pub window: usize,
    /// Maximum stereo PCM frames waiting between RT and the packager worker.
    #[builder(default = Defaults::BUFFER_FRAMES)]
    pub buffer_frames: NonZeroUsize,
    /// Maximum stereo PCM frames packaged during one worker tick.
    #[builder(default = Defaults::TICK_FRAMES)]
    pub tick_frames: NonZeroUsize,
    /// Maximum queued master-format generations waiting for the packager.
    #[builder(default = Defaults::GENERATION_CAPACITY)]
    pub generation_capacity: NonZeroUsize,
    /// Maximum tasks admitted to the broadcast dispatcher.
    #[builder(default = NonZeroUsize::MIN)]
    pub dispatcher_capacity: NonZeroUsize,
    /// Consecutive progress passes before the dispatcher yields.
    #[builder(default = Defaults::FAIRNESS_YIELD_INTERVAL)]
    pub fairness_yield_interval: NonZeroU32,
    /// Dispatcher park duration when the broadcast has no work.
    #[builder(default = Duration::from_millis(100))]
    pub idle_timeout: Duration,
    /// Threshold for reporting a slow packager tick.
    #[builder(default = Duration::from_millis(10))]
    pub slow_tick_threshold: Duration,
    /// Maximum consecutive packager ticks in one dispatcher visit.
    #[builder(default = NonZeroU32::MIN)]
    pub task_burst: NonZeroU32,
    /// Dispatcher wait duration between deferred RT wakes.
    #[builder(default = Duration::from_millis(2))]
    pub wait_timeout: Duration,
    /// Packager task priority.
    #[builder(default = Priority::new(0))]
    pub priority: Priority,
    /// Maximum compute jobs admitted for the packager task.
    #[builder(default = NonZeroUsize::MIN)]
    pub max_compute_tasks: NonZeroUsize,
    /// Maximum time a graceful stop waits for the bounded PCM tail.
    #[builder(default = Duration::from_secs(10))]
    pub stop_timeout: Duration,
}

struct Defaults;

impl Defaults {
    const BUFFER_FRAMES: NonZeroUsize = match NonZeroUsize::new(96_000) {
        Some(value) => value,
        None => unreachable!(),
    };
    const FAIRNESS_YIELD_INTERVAL: NonZeroU32 = match NonZeroU32::new(16) {
        Some(value) => value,
        None => unreachable!(),
    };
    const GENERATION_CAPACITY: NonZeroUsize = match NonZeroUsize::new(8) {
        Some(value) => value,
        None => unreachable!(),
    };
    const TICK_FRAMES: NonZeroUsize = match NonZeroUsize::new(4_096) {
        Some(value) => value,
        None => unreachable!(),
    };
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl BroadcastConfig {
    const MILLIS_PER_SECOND: u64 = 1_000;

    const MIN_TARGETS: u64 = 3;

    /// Copy this configuration with the measured master sample rate.
    #[must_use]
    pub fn with_sample_rate(&self, sample_rate: u32) -> Self {
        Self {
            sample_rate,
            ..self.clone()
        }
    }

    pub(crate) fn target_seconds(&self) -> BroadcastResult<u64> {
        Ok(self.target_ticks()?.div_ceil(u64::from(self.sample_rate)))
    }

    pub(crate) fn target_ticks(&self) -> BroadcastResult<u64> {
        u64::try_from(self.segment_target.as_millis())
            .ok()
            .and_then(|millis| millis.checked_mul(u64::from(self.sample_rate)))
            .map(|ticks| ticks / Self::MILLIS_PER_SECOND)
            .filter(|ticks| *ticks > 0)
            .ok_or(BroadcastError::InvalidConfig {
                field: "segment_target",
            })
    }

    pub(crate) fn validate(&self) -> BroadcastResult<()> {
        if self.codec != AudioCodec::AacLc || self.container != ContainerFormat::Adts {
            return Err(BroadcastError::UnsupportedProfile {
                codec: self.codec,
                container: self.container,
            });
        }
        if self.sample_rate == 0 {
            return Err(BroadcastError::InvalidConfig {
                field: "sample_rate",
            });
        }
        if self.channels == 0 {
            return Err(BroadcastError::InvalidConfig { field: "channels" });
        }
        if self.bit_rate == 0 {
            return Err(BroadcastError::InvalidConfig { field: "bit_rate" });
        }
        if self.window == 0 {
            return Err(BroadcastError::InvalidConfig { field: "window" });
        }
        if self.stop_timeout.is_zero() {
            return Err(BroadcastError::InvalidConfig {
                field: "stop_timeout",
            });
        }

        let window = u64::try_from(self.window)
            .map_err(|_| BroadcastError::InvalidConfig { field: "window" })?;
        let span_ts = window
            .checked_mul(self.target_ticks()?)
            .ok_or(BroadcastError::InvalidConfig { field: "window" })?;
        let minimum_ts = Self::MIN_TARGETS * self.target_seconds()? * u64::from(self.sample_rate);
        if span_ts < minimum_ts {
            return Err(BroadcastError::PlaylistTooShort {
                span_ts,
                minimum_ts,
                window: self.window,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    use kithara_stream::{AudioCodec, ContainerFormat};
    use kithara_test_utils::kithara;

    use super::BroadcastConfig;

    #[kithara::test(native, flash(false))]
    fn the_default_configuration_serves_a_long_enough_playlist() {
        BroadcastConfig::builder()
            .build()
            .validate()
            .expect("the defaults hold the RFC 8216 live window");
    }

    #[kithara::test(native, flash(false))]
    fn a_window_shorter_than_three_target_durations_is_rejected() {
        let short = BroadcastConfig::builder()
            .segment_target(Duration::from_millis(500))
            .window(5)
            .build();

        short
            .validate()
            .expect_err("five 500 ms segments span 2.5 s of a 1 s target duration");
        BroadcastConfig::builder()
            .segment_target(Duration::from_millis(500))
            .window(6)
            .build()
            .validate()
            .expect("six of them span exactly three target durations");
    }

    #[kithara::test(native, flash(false))]
    fn the_target_duration_rounds_the_segment_target_up_to_seconds() {
        let config = BroadcastConfig::builder()
            .segment_target(Duration::from_millis(1_500))
            .build();

        assert_eq!(config.target_seconds().expect("seconds"), 2);
        assert_eq!(config.target_ticks().expect("ticks"), 72_000);
    }

    #[kithara::test(native, flash(false))]
    fn zero_audio_is_rejected() {
        assert!(
            BroadcastConfig::builder()
                .sample_rate(0)
                .build()
                .validate()
                .is_err()
        );
        assert!(
            BroadcastConfig::builder()
                .channels(0)
                .build()
                .validate()
                .is_err()
        );
        assert!(
            BroadcastConfig::builder()
                .segment_target(Duration::ZERO)
                .build()
                .validate()
                .is_err()
        );
    }

    #[kithara::test(native, flash(false))]
    fn measured_rate_preserves_the_selected_profile() {
        let configured = BroadcastConfig::builder()
            .sample_rate(44_100)
            .codec(AudioCodec::AacLc)
            .container(ContainerFormat::Adts)
            .bit_rate(192_000)
            .build();

        let measured = configured.with_sample_rate(48_000);

        assert_eq!(measured.sample_rate, 48_000);
        assert_eq!(measured.codec, configured.codec);
        assert_eq!(measured.container, configured.container);
        assert_eq!(measured.bit_rate, configured.bit_rate);
    }
}
