use std::fmt;

use bon::Builder;
use kithara_abr::AbrController;
use kithara_decode::GaplessMode;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::{BeatGridId, StretchControls};
use struct_patch::Patch;

use crate::{
    PlayWorker,
    engine::{EngineSettings, EngineSettingsPatch},
    session::SessionDispatcher,
};

fn allocate_grid_id() -> BeatGridId {
    let Ok(id) = BeatGridId::allocate() else {
        panic!("process-wide beat-grid identity space is exhausted");
    };
    id
}

/// Player-level knobs a configuration document can override. Engine knobs
/// (`sample_rate`, `max_slots`, `eq_layout`) live on the nested
/// [`EngineSettings`] rather than being repeated here: two settings structs
/// declaring the same value would be a second mutable source of truth for
/// one number.
#[derive(Clone, Debug, PartialEq, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct PlayerSettings {
    /// How resources created for this player trim leading/trailing audio.
    /// Document-unreachable until `GaplessMode` derives `Deserialize`.
    #[builder(default)]
    #[patch(skip)]
    pub gapless_mode: GaplessMode,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling the block. Offline (faster-than-real-time) harnesses opt
    /// in so rendered output never stretches with inserted silence while the
    /// decode worker catches up. Real-time hosts must keep the default
    /// (`false`): the audio callback can never block. Not a document key:
    /// the shipped binary is a real-time host, and only the offline test
    /// harness sets this, from Rust.
    #[builder(default)]
    #[patch(skip)]
    pub block_on_underrun: bool,
    /// Built-in auto-advance handler. The queue overwrites this for every
    /// queue-driven player at construction, so it is not a document key.
    /// See `crates/kithara-play/CONTEXT.md` for the owning contract.
    #[builder(default = true)]
    #[patch(skip)]
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
    #[patch(skip)]
    pub prefetch_duration: f32,
    /// Engine-level knobs. See [`EngineSettings`] for what a document may
    /// say under `player.engine`.
    #[builder(default)]
    #[patch(name = "EngineSettingsPatch")]
    pub engine: EngineSettings,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for the player.
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
    /// Player- and engine-level knobs a configuration document can override.
    /// See [`PlayerSettings`].
    #[builder(default)]
    pub(crate) settings: PlayerSettings,
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
            settings: self.settings.clone(),
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
            .field("settings", &self.settings)
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::PlayerSettings;

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let settings = PlayerSettings::default();

        assert!(!settings.block_on_underrun);
        assert!(settings.auto_advance_enabled);
        assert!((settings.crossfade_duration - 1.0).abs() < f32::EPSILON);
        assert!((settings.default_rate - 1.0).abs() < f32::EPSILON);
        assert!((settings.prefetch_duration - 3.5).abs() < f32::EPSILON);
        assert_eq!(settings.engine.max_slots, 4);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::PlayerSettings;

    /// `deny_unknown_fields` arrives through `#[patch(attribute(...))]`,
    /// which emits its token stream verbatim. A typo there would generate a
    /// patch that accepts anything, and neither the compiler nor clippy
    /// would say a word -- only a bogus key proves the attribute survived
    /// generation. `slot_ceiling` is not a prefix of any real field (unlike
    /// `max_slot`, which would pass this assertion vacuously because the
    /// error message lists the real `max_slots` field among the valid
    /// names).
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<super::PlayerSettingsPatch>("slot_ceiling: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("slot_ceiling"), "{error}");
    }

    /// `prefetch_duration` is a real field on `PlayerSettings` but must not
    /// be document-reachable: the queue always overwrites it at
    /// construction (see the field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_queue_owned_prefetch_field_is_not_a_document_key() {
        let error =
            serde_yaml_ng::from_str::<super::PlayerSettingsPatch>("prefetch_duration: 8.0\n")
                .expect_err("a queue-owned field must not be settable from a document");

        assert!(error.to_string().contains("prefetch_duration"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_crossfade_it_names() {
        let patch: super::PlayerSettingsPatch =
            serde_yaml_ng::from_str("crossfade_duration: 2.0\n").expect("the document types");
        let mut settings = PlayerSettings::default();
        // Seeded off the default (1.0) so a whole-struct `apply` that resets
        // every unnamed field to `Default::default()` cannot pass this
        // assertion by coincidence.
        settings.default_rate = 2.5;

        settings.apply(patch);

        assert!((settings.crossfade_duration - 2.0).abs() < f32::EPSILON);
        assert!(
            (settings.default_rate - 2.5).abs() < f32::EPSILON,
            "a silent field must keep its seeded value, not reset to default"
        );
    }

    /// A document reaching `player.engine.sample_rate` lands on the exact
    /// same `EngineSettings` that `PlayerConfig` hands to `EngineConfig`, so
    /// the two settings trees cannot drift into two spellings of one value.
    #[kithara::test(native, flash(false))]
    fn a_nested_engine_patch_reaches_the_player() {
        let patch: super::PlayerSettingsPatch =
            serde_yaml_ng::from_str("engine:\n  sample_rate: 48000\n").expect("the document types");
        let mut settings = PlayerSettings::default();
        // Seeded off the default (4) so a whole-struct replacement of the
        // nested `engine` field (rather than a field-by-field merge) would
        // go red here instead of passing by coincidence.
        settings.engine.max_slots = 8;

        settings.apply(patch);

        assert_eq!(settings.engine.sample_rate.get(), 48_000);
        assert_eq!(
            settings.engine.max_slots, 8,
            "a sibling field inside the nested settings must survive the patch"
        );
    }

    /// `block_on_underrun` is a real field on `PlayerSettings` but must not
    /// be document-reachable: the shipped binary is a real-time host whose
    /// audio callback can never block (see the field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_realtime_unsafe_block_on_underrun_field_is_not_a_document_key() {
        let error =
            serde_yaml_ng::from_str::<super::PlayerSettingsPatch>("block_on_underrun: true\n")
                .expect_err(
                    "a field that can park the audio callback must not be document-settable",
                );

        assert!(error.to_string().contains("block_on_underrun"), "{error}");
    }
}
