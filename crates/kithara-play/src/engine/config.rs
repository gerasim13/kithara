use std::{fmt, num::NonZeroU32};

use bon::Builder;
use kithara_bufpool::PoolRegion;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::BeatGridId;
use struct_patch::Patch;

use crate::{
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    session::SessionDispatcher,
};

const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// Engine-level knobs a configuration document can override. Extracted out of
/// [`EngineConfig`] so a document reaches exactly these tunables and never the
/// per-call wiring (`grid_id`, `cancel`, `session`, `pools`) that stays on
/// [`EngineConfig`] itself.
///
/// `PlayerConfig` builds an `EngineConfig` from a player's own settings, so
/// `PlayerSettings` holds one of these under `engine` rather than repeating
/// `sample_rate`, `max_slots` and `eq_layout` as a second copy: two settings
/// structs declaring the same three values would be a second mutable source
/// of truth for one number.
#[derive(Clone, Debug, PartialEq, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "EngineSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct EngineSettings {
    /// EQ band layout per player. Default: 10-band log-spaced. Not a
    /// document key: every construction site in the workspace derives this
    /// from a generator (`generate_log_spaced_bands`), and a custom layout
    /// is installed at runtime through `PlayerImpl::set_eq_layout` rather
    /// than through config.
    #[patch(skip)]
    #[builder(default = generate_log_spaced_bands(10))]
    pub eq_layout: Vec<EqBandConfig>,
    /// Number of output channels. Default: 2 (stereo). Not a document key:
    /// the only reader is a startup log line, so a document value would
    /// change nothing the engine actually does.
    #[patch(skip)]
    #[builder(default = 2)]
    pub channels: u16,
    /// Sample rate passed to the runtime backend as a hint. Default: 44100.
    /// Offline/test harnesses set this to drive deterministic render at a
    /// known rate.
    #[builder(default = DEFAULT_SAMPLE_RATE)]
    pub sample_rate: NonZeroU32,
    /// Maximum concurrent slots in the engine. Default: 4.
    #[builder(default = 4)]
    pub max_slots: usize,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for the audio engine.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct EngineConfig<S> {
    /// Stable synchronization identity of the owning player.
    pub(crate) grid_id: BeatGridId,
    /// Master cancel token for the engine. The worker scheduler derives a
    /// `child()` so its produce-core's lock-free `is_cancelled()` read
    /// observes a master cancel.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional pre-bound dispatcher for isolated harnesses. Production
    /// engines receive their session when the owning Player enters a Host.
    pub(crate) session: Option<Arc<dyn SessionDispatcher<S>>>,
    /// Typed pool facade for audio-thread scratch buffers.
    pub(crate) pools: PoolRegion<S>,
    /// Engine-level knobs a configuration document can override. See
    /// [`EngineSettings`] for what a document may say.
    #[builder(default)]
    pub(crate) settings: EngineSettings,
}

impl<S> Clone for EngineConfig<S> {
    fn clone(&self) -> Self {
        Self {
            grid_id: self.grid_id,
            cancel: self.cancel.clone(),
            session: self.session.clone(),
            pools: self.pools.clone(),
            settings: self.settings.clone(),
        }
    }
}

impl<S> fmt::Debug for EngineConfig<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineConfig")
            .field("settings", &self.settings)
            .field("pools", &self.pools)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::EngineSettings;

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let settings = EngineSettings::default();

        assert_eq!(settings.channels, 2);
        assert_eq!(settings.sample_rate.get(), 44_100);
        assert_eq!(settings.max_slots, 4);
        assert_eq!(settings.eq_layout.len(), 10);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::EngineSettings;

    /// `deny_unknown_fields` arrives through `#[patch(attribute(...))]`,
    /// which emits its token stream verbatim. A typo there would generate a
    /// patch that accepts anything, and neither the compiler nor clippy
    /// would say a word -- only a bogus key proves the attribute survived
    /// generation.
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<super::EngineSettingsPatch>("slot_ceiling: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("slot_ceiling"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_field_it_names() {
        let patch: super::EngineSettingsPatch =
            serde_yaml_ng::from_str("max_slots: 8\n").expect("the document types");
        let mut settings = EngineSettings::default();
        // Seeded off the default (2) so a whole-struct `apply` that resets
        // every unnamed field to `Default::default()` cannot pass this
        // assertion by coincidence.
        settings.channels = 5;

        settings.apply(patch);

        assert_eq!(settings.max_slots, 8);
        assert_eq!(
            settings.channels, 5,
            "a silent field must keep its seeded value, not reset to default"
        );
    }

    /// `eq_layout` is a real field on `EngineSettings` but must not be
    /// document-reachable: every construction site derives it from
    /// `generate_log_spaced_bands`, and a custom layout is installed at
    /// runtime through `PlayerImpl::set_eq_layout` (see the field's doc
    /// comment).
    #[kithara::test(native, flash(false))]
    fn the_generator_owned_eq_layout_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<super::EngineSettingsPatch>("eq_layout: []\n")
            .expect_err("a generator-owned field must not be settable from a document");

        assert!(error.to_string().contains("eq_layout"), "{error}");
    }

    /// `channels` is a real field on `EngineSettings` but must not be
    /// document-reachable: its only reader is a startup log line, so a
    /// document value would change nothing the engine actually does (see
    /// the field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_unread_channels_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<super::EngineSettingsPatch>("channels: 1\n")
            .expect_err("a field with no consumer must not be settable from a document");

        assert!(error.to_string().contains("channels"), "{error}");
    }
}
