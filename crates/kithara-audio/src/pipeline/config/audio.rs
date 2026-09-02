use std::num::{NonZeroU32, NonZeroUsize};

use bon::Builder;
use kithara_events::EventBus;
use kithara_platform::CancelToken;
use kithara_resampler::{NoResamplerBackend, ResamplerBackend};
use kithara_stream::{MediaInfo, StreamType};
use struct_patch::Patch;

use crate::{pipeline::config::AudioDecoderConfig, traits::AudioObserver};

struct Consts;

impl Consts {
    /// PCM ring depth, ~100 ms per chunk. wasm needs a deeper ring because
    /// its worker is scheduled coarsely.
    #[cfg(not(target_arch = "wasm32"))]
    const PCM_BUFFER_CHUNKS: usize = 10;
    #[cfg(target_arch = "wasm32")]
    const PCM_BUFFER_CHUNKS: usize = 32;
    /// Chunks buffered before preload readiness is signalled.
    const PRELOAD_CHUNKS: usize = 3;
}

/// The consumer's thread capability: how it wakes the decode worker after
/// draining its ring, and how its reader-born events reach the bus.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConsumerWakeMode {
    /// Arm a coalesced scheduler pass without signaling a thread gate.
    #[default]
    RealtimeDeferred,
    /// Unpark the worker's thread, for a consumer off the real-time thread.
    /// Marks the consumer's read path as free to block, so reader-born events
    /// publish inline instead of waiting for a scheduler-shell flush.
    ImmediateOffRt,
}

/// Audio-pipeline knobs a configuration document can override. Extracted out
/// of [`AudioConfig`] so a document reaches exactly these tunables and never
/// the per-call wiring (`stream`, `decoder`, `bus`, `cancel`, `hint`,
/// `media_info`, `observer`) that stays on [`AudioConfig`] itself.
#[derive(Clone, Debug, PartialEq, Builder, Patch, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[patch(name = "AudioSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct AudioSettings {
    /// Number of chunks to buffer before signaling preload readiness.
    #[field(get, copy)]
    #[builder(default = NonZeroUsize::new(Consts::PRELOAD_CHUNKS).expect("preload chunk count is non-zero"))]
    pub preload_chunks: NonZeroUsize,
    /// Target sample rate of the audio host (for resampling). Not a document
    /// key: this is the rate the audio host actually opened, and the
    /// resource-preparation step that shares a player's engine always
    /// overwrites it with the engine's master or configured rate. A document
    /// value would be overwritten by the first host that disagrees with it.
    #[field(get, copy)]
    #[patch(skip)]
    pub host_sample_rate: Option<NonZeroU32>,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling. Not a document key: the shipped binary is a real-time
    /// host whose audio callback can never block; only an offline harness or
    /// a player's own session policy sets this explicitly.
    #[field(get, copy)]
    #[builder(default)]
    #[patch(skip)]
    pub block_on_underrun: bool,
    /// Consumer wake capability for ring pops and reader-event delivery. Not
    /// a document key: a player-managed resource has this value overwritten
    /// with its session's wake policy, and declaring `ImmediateOffRt` here
    /// would make a player-bound resource publish reads inline on the render
    /// callback.
    #[field(get, copy)]
    #[builder(default)]
    #[patch(skip)]
    pub consumer_wake_mode: ConsumerWakeMode,
    /// PCM buffer size in chunks (~100ms per chunk = 10 chunks ≈ 1s).
    /// Default: 10 on native, 32 on wasm32.
    #[field(get, copy)]
    #[builder(default = Consts::PCM_BUFFER_CHUNKS)]
    pub audio_buffer_chunks: usize,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for audio pipeline with stream config.
///
/// Generic over `StreamType` to include stream-specific configuration.
/// Combines stream config and audio pipeline settings into a single builder.
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(start_fn = for_stream)]
#[non_exhaustive]
#[fieldwork(opt_in, get)]
pub struct AudioConfig<T: StreamType, B = NoResamplerBackend> {
    /// Stream configuration (`HlsConfig`, `FileConfig`, etc.)
    #[builder(start_fn)]
    #[field(get)]
    pub(crate) stream: T::Config,
    /// Decoder construction settings, including decoder-side resampling.
    #[builder(default)]
    #[field(get)]
    pub(crate) decoder: AudioDecoderConfig<B>,
    /// Audio-pipeline knobs a configuration document can override. See
    /// [`AudioSettings`] for what a document may say.
    #[builder(default)]
    pub(crate) settings: AudioSettings,
    /// Unified event bus (optional — if not provided, one is created internally).
    #[builder(name = events)]
    pub(crate) bus: Option<EventBus>,
    /// Master cancel token for the audio pipeline.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional format hint (file extension like "mp3", "wav")
    pub(crate) hint: Option<String>,
    /// Media info hint for format detection
    pub(crate) media_info: Option<MediaInfo>,
    /// Optional bounded, nonblocking observer of decoder-output PCM.
    /// [`kithara_signal::AudioChunk::meta`] describes its post-conversion format;
    /// it runs before playback effects and owns any asynchronous copy.
    pub(crate) observer: Option<Box<dyn AudioObserver>>,
}

impl<T: StreamType, B> AudioConfig<T, B> {
    delegate::delegate! {
        to self.settings {
            /// Number of chunks to buffer before signaling preload readiness.
            #[must_use]
            pub fn preload_chunks(&self) -> NonZeroUsize;
            /// Target sample rate of the audio host (for resampling).
            #[must_use]
            pub fn host_sample_rate(&self) -> Option<NonZeroU32>;
            /// Whether audio-thread reads block on a producer-ring underrun.
            #[must_use]
            pub fn block_on_underrun(&self) -> bool;
            /// Consumer wake capability for ring pops and reader-event delivery.
            #[must_use]
            pub fn consumer_wake_mode(&self) -> ConsumerWakeMode;
            /// PCM buffer size in chunks.
            #[must_use]
            pub fn audio_buffer_chunks(&self) -> usize;
        }
    }
}

impl<T, B> AudioConfig<T, B>
where
    T: StreamType,
    B: ResamplerBackend,
{
    /// Return the configured event bus.
    #[must_use]
    pub const fn bus(&self) -> Option<&EventBus> {
        self.bus.as_ref()
    }

    /// Return the configured cancellation token.
    #[must_use]
    pub const fn cancel(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    /// Return the optional format hint.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Return the media information hint.
    #[must_use]
    pub const fn media_info(&self) -> Option<&MediaInfo> {
        self.media_info.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{AudioSettings, Consts, ConsumerWakeMode};

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let settings = AudioSettings::default();

        assert_eq!(settings.preload_chunks.get(), Consts::PRELOAD_CHUNKS);
        assert_eq!(settings.audio_buffer_chunks, Consts::PCM_BUFFER_CHUNKS);
        assert_eq!(
            settings.consumer_wake_mode,
            ConsumerWakeMode::RealtimeDeferred
        );
        assert!(!settings.block_on_underrun);
        assert!(settings.host_sample_rate.is_none());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{AudioSettings, AudioSettingsPatch, ConsumerWakeMode};

    /// `deny_unknown_fields` arrives through `#[patch(attribute(...))]`,
    /// which emits its token stream verbatim -- only a bogus key proves the
    /// attribute survived generation. `preload_chunks` and
    /// `audio_buffer_chunks` are the patch's only declared fields, and
    /// `headroom` is neither a substring of either nor contains one, so the
    /// assertion cannot pass off serde's list of valid names.
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<AudioSettingsPatch>("headroom: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("headroom"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_two_live_document_keys() {
        let patch: AudioSettingsPatch =
            serde_yaml_ng::from_str("preload_chunks: 8\naudio_buffer_chunks: 20\n")
                .expect("the document types");
        let mut settings = AudioSettings::default();
        // Seeded off non-default values so a whole-struct `apply` that resets
        // every unnamed field to `Default::default()` cannot pass these
        // assertions by coincidence.
        settings.consumer_wake_mode = ConsumerWakeMode::ImmediateOffRt;
        settings.block_on_underrun = true;
        settings.host_sample_rate = NonZeroU32::new(48_000);

        settings.apply(patch);

        assert_eq!(settings.preload_chunks.get(), 8);
        assert_eq!(settings.audio_buffer_chunks, 20);
        assert_eq!(
            settings.consumer_wake_mode,
            ConsumerWakeMode::ImmediateOffRt,
            "a skipped field must keep its seeded value, not reset to default"
        );
        assert!(
            settings.block_on_underrun,
            "a skipped field must keep its seeded value, not reset to default"
        );
        assert_eq!(settings.host_sample_rate, NonZeroU32::new(48_000));
    }

    /// `consumer_wake_mode` is overwritten for every player-managed resource
    /// (see the field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_wake_mode_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<AudioSettingsPatch>(
            "consumer_wake_mode: immediate_off_rt\n",
        )
        .expect_err(
            "a capability that moves reads onto the render callback is not document-settable",
        );

        assert!(error.to_string().contains("consumer_wake_mode"), "{error}");
    }

    /// `block_on_underrun` can park a real-time audio callback (see the
    /// field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_block_on_underrun_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<AudioSettingsPatch>("block_on_underrun: true\n")
            .expect_err("a field that can park the audio callback must not be document-settable");

        assert!(error.to_string().contains("block_on_underrun"), "{error}");
    }

    /// `host_sample_rate` is the rate the audio host actually opened (see the
    /// field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_runtime_owned_host_sample_rate_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<AudioSettingsPatch>("host_sample_rate: 48000\n")
            .expect_err("the audio host owns its own rate");

        assert!(error.to_string().contains("host_sample_rate"), "{error}");
    }
}
