use std::{fmt, num::NonZeroU32};

use bon::Builder;
use kithara_abr::AbrController;
use kithara_decode::GaplessMode;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::{BeatGridId, StretchControls};
use serde::Deserialize;
use struct_patch::Patch;

use crate::{
    PlayWorker,
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    engine::EngineConfigPatch,
    session::SessionDispatcher,
};

const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

fn allocate_grid_id() -> BeatGridId {
    let Ok(id) = BeatGridId::allocate() else {
        panic!("process-wide beat-grid identity space is exhausted");
    };
    id
}

/// What a configuration document may say about [`PlayerConfig`].
///
/// Hand-written rather than derived: `struct-patch` copies a struct's generics
/// and where-clause verbatim onto the patch it generates, so a patch of a
/// generic configuration whose generic-carrying fields are skipped has a type
/// parameter no field uses and does not compile. The fields absent here are
/// absent on purpose — see the [`PlayerConfig`] fields documented "not a
/// document key" — and `deny_unknown_fields` refuses them by name rather than
/// dropping them silently.
///
/// The nested `engine` key is where a document names the values this player
/// hands to the [`EngineConfig`] it builds; they land on this config's own
/// fields, which is the single place they live until that engine exists.
///
/// `Deserialize` only, never `Serialize`: by the time a patch is typed its
/// references are resolved, so the tree it merges into holds secrets in the
/// clear.
///
/// [`EngineConfig`]: crate::EngineConfig
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct PlayerConfigPatch {
    /// See [`PlayerConfig::gapless_mode`].
    pub gapless_mode: Option<GaplessMode>,
    /// See [`PlayerConfig::crossfade_duration`].
    pub crossfade_duration: Option<f32>,
    /// See [`PlayerConfig::default_rate`].
    pub default_rate: Option<f32>,
    /// What the document says about the engine this player builds. See
    /// [`PlayerConfig::sample_rate`] and [`PlayerConfig::max_slots`].
    pub engine: EngineConfigPatch,
}

impl<S> Patch<PlayerConfigPatch> for PlayerConfig<S> {
    fn apply(&mut self, patch: PlayerConfigPatch) {
        if let Some(gapless_mode) = patch.gapless_mode {
            self.gapless_mode = gapless_mode;
        }
        if let Some(crossfade_duration) = patch.crossfade_duration {
            self.crossfade_duration = crossfade_duration;
        }
        if let Some(default_rate) = patch.default_rate {
            self.default_rate = default_rate;
        }
        if let Some(sample_rate) = patch.engine.sample_rate {
            self.sample_rate = sample_rate;
        }
        if let Some(max_slots) = patch.engine.max_slots {
            self.max_slots = max_slots;
        }
    }

    fn into_patch(self) -> PlayerConfigPatch {
        PlayerConfigPatch {
            gapless_mode: Some(self.gapless_mode),
            crossfade_duration: Some(self.crossfade_duration),
            default_rate: Some(self.default_rate),
            engine: EngineConfigPatch {
                sample_rate: Some(self.sample_rate),
                max_slots: Some(self.max_slots),
            },
        }
    }

    fn into_patch_by_diff(self, previous: Self) -> PlayerConfigPatch {
        PlayerConfigPatch {
            gapless_mode: (self.gapless_mode != previous.gapless_mode).then_some(self.gapless_mode),
            crossfade_duration: (self.crossfade_duration != previous.crossfade_duration)
                .then_some(self.crossfade_duration),
            default_rate: (self.default_rate != previous.default_rate).then_some(self.default_rate),
            engine: EngineConfigPatch {
                sample_rate: (self.sample_rate != previous.sample_rate).then_some(self.sample_rate),
                max_slots: (self.max_slots != previous.max_slots).then_some(self.max_slots),
            },
        }
    }

    fn new_empty_patch() -> PlayerConfigPatch {
        PlayerConfigPatch::default()
    }
}

/// Configuration for the player.
///
/// Holds the player's own tunables, the engine values it hands to the
/// [`EngineConfig`] it builds, and the per-call wiring a caller passes in.
/// [`PlayerConfigPatch`] is what a configuration document may say about it.
///
/// [`EngineConfig`]: crate::EngineConfig
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct PlayerConfig<S> {
    /// Stable synchronization-group identity owned by this player.
    #[builder(default = allocate_grid_id())]
    pub(crate) grid_id: BeatGridId,
    /// Per-deck time-stretch control handle, shared with the UI and the
    /// worker Warp chain (see `kithara_warp::StretchControls`).
    #[builder(default = StretchControls::new(1.0))]
    pub(crate) timestretch: Arc<StretchControls>,
    /// Explicit shared playback worker. Its pools and cancellation lifetime
    /// are configured once in [`crate::PlayWorkerConfig`].
    pub(crate) worker: PlayWorker<S>,
    /// How resources created for this player trim leading/trailing audio.
    #[builder(default)]
    pub gapless_mode: GaplessMode,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling the block. Offline (faster-than-real-time) harnesses opt
    /// in so rendered output never stretches with inserted silence while the
    /// decode worker catches up. Real-time hosts must keep the default
    /// (`false`): the audio callback can never block. Not a document key:
    /// the shipped binary is a real-time host, and only the offline test
    /// harness sets this, from Rust.
    #[builder(default)]
    pub block_on_underrun: bool,
    /// Built-in auto-advance handler. The queue overwrites this for every
    /// queue-driven player at construction, so it is not a document key.
    /// See `crates/kithara-play/CONTEXT.md` for the owning contract.
    #[builder(default = true)]
    pub auto_advance_enabled: bool,
    /// Crossfade duration in seconds. Default: 1.0.
    #[builder(default = 1.0)]
    pub crossfade_duration: f32,
    /// Default playback-rate target (1.0 = normal). Default: 1.0.
    #[builder(default = 1.0)]
    pub default_rate: f32,
    /// Secondary lead time before EOF at which the next queued item is
    /// loaded. The queue overwrites this for every queue-driven player at
    /// construction, so it is not a document key. See
    /// `crates/kithara-play/CONTEXT.md` for the owning contract.
    #[builder(default = 3.5)]
    pub prefetch_duration: f32,
    /// Sample rate handed to the engine this player builds, and to the
    /// player's own sync identity. Default: 44100. A document names it under
    /// `player.engine.sample_rate`.
    #[builder(default = DEFAULT_SAMPLE_RATE)]
    pub sample_rate: NonZeroU32,
    /// Maximum concurrent slots of the engine this player builds. Default: 4.
    /// A document names it under `player.engine.max_slots`.
    #[builder(default = 4)]
    pub max_slots: usize,
    /// EQ band layout handed to the engine this player builds. Not a document
    /// key: every construction site derives it from a generator, and a custom
    /// layout is installed at runtime through `PlayerImpl::set_eq_layout`.
    #[builder(default = generate_log_spaced_bands(10))]
    pub eq_layout: Vec<EqBandConfig>,
    /// Shared ABR controller. When `None`, a default one is created.
    pub(crate) abr: Option<Arc<AbrController>>,
    /// Root event bus for this player.
    pub(crate) bus: Option<EventBus>,
    /// Master cancel token for this player.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional pre-bound session for isolated harnesses. Production players
    /// are constructed unbound and attached exactly once by their Host.
    pub(crate) session: Option<Arc<dyn SessionDispatcher<S>>>,
}

impl<S> Clone for PlayerConfig<S> {
    fn clone(&self) -> Self {
        Self {
            grid_id: self.grid_id,
            timestretch: Arc::clone(&self.timestretch),
            worker: self.worker.clone(),
            gapless_mode: self.gapless_mode,
            block_on_underrun: self.block_on_underrun,
            auto_advance_enabled: self.auto_advance_enabled,
            crossfade_duration: self.crossfade_duration,
            default_rate: self.default_rate,
            prefetch_duration: self.prefetch_duration,
            sample_rate: self.sample_rate,
            max_slots: self.max_slots,
            eq_layout: self.eq_layout.clone(),
            abr: self.abr.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            session: self.session.clone(),
        }
    }
}

impl<S> fmt::Debug for PlayerConfig<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlayerConfig")
            .field("gapless_mode", &self.gapless_mode)
            .field("crossfade_duration", &self.crossfade_duration)
            .field("default_rate", &self.default_rate)
            .field("sample_rate", &self.sample_rate)
            .field("max_slots", &self.max_slots)
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::PlayerConfig;
    use crate::{
        PlayWorker, PlayWorkerConfig,
        test_pools::{TestPools, pools},
    };

    pub(super) fn config() -> PlayerConfig<TestPools> {
        PlayerConfig::builder()
            .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
            .build()
    }

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let config = config();

        assert!(!config.block_on_underrun);
        assert!(config.auto_advance_enabled);
        assert!((config.crossfade_duration - 1.0).abs() < f32::EPSILON);
        assert!((config.default_rate - 1.0).abs() < f32::EPSILON);
        assert!((config.prefetch_duration - 3.5).abs() < f32::EPSILON);
        assert_eq!(config.sample_rate.get(), 44_100);
        assert_eq!(config.max_slots, 4);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{GaplessMode, PlayerConfigPatch, tests::config};

    /// `deny_unknown_fields` is hand-written here rather than emitted by
    /// `#[patch(attribute(...))]`, so a bogus key is what proves it is on the
    /// type at all. `slot_ceiling` is not a prefix of any real field (unlike
    /// `max_slot`, which would pass this assertion vacuously because the
    /// error message lists the real `max_slots` field among the valid names).
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<PlayerConfigPatch>("slot_ceiling: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("slot_ceiling"), "{error}");
    }

    /// `prefetch_duration` is a real field on [`PlayerConfig`] but must not
    /// be document-reachable: the queue always overwrites it at construction
    /// (see the field's doc comment).
    ///
    /// [`PlayerConfig`]: super::PlayerConfig
    #[kithara::test(native, flash(false))]
    fn the_queue_owned_prefetch_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<PlayerConfigPatch>("prefetch_duration: 8.0\n")
            .expect_err("a queue-owned field must not be settable from a document");

        assert!(error.to_string().contains("prefetch_duration"), "{error}");
    }

    /// `block_on_underrun` is a real field on [`PlayerConfig`] but must not
    /// be document-reachable: the shipped binary is a real-time host whose
    /// audio callback can never block (see the field's doc comment).
    ///
    /// [`PlayerConfig`]: super::PlayerConfig
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_block_on_underrun_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<PlayerConfigPatch>("block_on_underrun: true\n")
            .expect_err("a field that can park the audio callback must not be document-settable");

        assert!(error.to_string().contains("block_on_underrun"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_crossfade_it_names() {
        let patch: PlayerConfigPatch =
            serde_yaml_ng::from_str("crossfade_duration: 2.0\n").expect("the document types");
        let mut config = config();
        // Seeded off the default (1.0) so a whole-struct `apply` that resets
        // every unnamed field to `Default::default()` cannot pass this
        // assertion by coincidence.
        config.default_rate = 2.5;

        config.apply(patch);

        assert!((config.crossfade_duration - 2.0).abs() < f32::EPSILON);
        assert!(
            (config.default_rate - 2.5).abs() < f32::EPSILON,
            "a silent field must keep its seeded value, not reset to default"
        );
    }

    /// A document reaching `player.engine.sample_rate` lands on the very
    /// field this player hands to the engine it builds, so the value the
    /// document names and the value the engine receives cannot drift apart.
    #[kithara::test(native, flash(false))]
    fn a_nested_engine_patch_reaches_the_player() {
        let patch: PlayerConfigPatch =
            serde_yaml_ng::from_str("engine:\n  sample_rate: 48000\n").expect("the document types");
        let mut config = config();
        // Seeded off the default (4) so a whole-struct replacement of the
        // nested `engine` key (rather than a field-by-field merge) would go
        // red here instead of passing by coincidence.
        config.max_slots = 8;

        config.apply(patch);

        assert_eq!(config.sample_rate.get(), 48_000);
        assert_eq!(
            config.max_slots, 8,
            "a sibling field inside the nested key must survive the patch"
        );
    }

    /// `gapless_mode` was skipped until `GaplessMode` derived `Deserialize`.
    /// Now that it does, a document naming it must reach the configuration
    /// without disturbing a sibling field.
    #[kithara::test(native, flash(false))]
    fn a_gapless_mode_patch_reaches_the_player() {
        let patch: PlayerConfigPatch = serde_yaml_ng::from_str("gapless_mode:\n  mode: disabled\n")
            .expect("the document types");
        let mut config = config();
        // `disabled` differs from the `MediaOnly` default, so only the patch
        // can produce it. The sibling is seeded off its own default (1.0) so a
        // whole-struct reset would go red here rather than pass by coincidence.
        config.crossfade_duration = 2.5;

        config.apply(patch);

        assert_eq!(config.gapless_mode, GaplessMode::Disabled);
        assert!(
            (config.crossfade_duration - 2.5).abs() < f32::EPSILON,
            "a sibling field must survive the patch"
        );
    }
}
