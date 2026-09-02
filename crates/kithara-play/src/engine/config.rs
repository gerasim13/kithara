use std::{fmt, num::NonZeroU32};

use bon::Builder;
use kithara_bufpool::PoolRegion;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::BeatGridId;
use serde::Deserialize;

use crate::{
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    session::SessionDispatcher,
};

const DEFAULT_SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// What a configuration document may say about [`EngineConfig`], under a
/// player's `engine` key.
///
/// Nothing merges it into an [`EngineConfig`] directly: a player owns the
/// resolved `sample_rate` and `max_slots` and hands them to
/// [`EngineConfig::builder`], so `PlayerConfig`'s own patch carries this one
/// and applies it to the player. The per-call wiring (`grid_id`, `cancel`,
/// `session`, `pools`), `eq_layout` and `channels` are absent on purpose, and
/// `deny_unknown_fields` refuses them by name rather than dropping them
/// silently.
///
/// `Deserialize` only, never `Serialize`: by the time a patch is typed its
/// references are resolved, so the tree it merges into holds secrets in the
/// clear.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct EngineConfigPatch {
    /// See [`EngineConfig::sample_rate`].
    pub sample_rate: Option<NonZeroU32>,
    /// See [`EngineConfig::max_slots`].
    pub max_slots: Option<usize>,
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
    /// EQ band layout per player. Default: 10-band log-spaced. Not a
    /// document key: every construction site in the workspace derives this
    /// from a generator (`generate_log_spaced_bands`), and a custom layout
    /// is installed at runtime through `PlayerImpl::set_eq_layout` rather
    /// than through config.
    #[builder(default = generate_log_spaced_bands(10))]
    pub(crate) eq_layout: Vec<EqBandConfig>,
    /// Number of output channels. Default: 2 (stereo). Not a document key:
    /// the only reader is a startup log line, so a document value would
    /// change nothing the engine actually does.
    #[builder(default = 2)]
    pub(crate) channels: u16,
    /// Sample rate passed to the runtime backend as a hint. Default: 44100.
    /// Offline/test harnesses set this to drive deterministic render at a
    /// known rate.
    #[builder(default = DEFAULT_SAMPLE_RATE)]
    pub(crate) sample_rate: NonZeroU32,
    /// Maximum concurrent slots in the engine. Default: 4.
    #[builder(default = 4)]
    pub(crate) max_slots: usize,
}

impl<S> Clone for EngineConfig<S> {
    fn clone(&self) -> Self {
        Self {
            grid_id: self.grid_id,
            cancel: self.cancel.clone(),
            session: self.session.clone(),
            pools: self.pools.clone(),
            eq_layout: self.eq_layout.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            max_slots: self.max_slots,
        }
    }
}

impl<S> fmt::Debug for EngineConfig<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineConfig")
            .field("sample_rate", &self.sample_rate)
            .field("max_slots", &self.max_slots)
            .field("channels", &self.channels)
            .field("pools", &self.pools)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;

    use super::{EngineConfigPatch, NonZeroU32};

    /// `deny_unknown_fields` is hand-written here rather than emitted by
    /// `#[patch(attribute(...))]`, so a bogus key is what proves it is on the
    /// type at all. `slot_ceiling` is not a prefix of any real field (unlike
    /// `max_slot`, which would pass this assertion vacuously because the
    /// error message lists the real `max_slots` field among the valid names).
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<EngineConfigPatch>("slot_ceiling: 8\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("slot_ceiling"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn the_document_names_both_live_keys() {
        let patch: EngineConfigPatch =
            serde_yaml_ng::from_str("sample_rate: 48000\nmax_slots: 8\n")
                .expect("the document types");

        assert_eq!(patch.sample_rate.map(NonZeroU32::get), Some(48_000));
        assert_eq!(patch.max_slots, Some(8));
    }

    /// A key the document does not name stays unset, so the merge has nothing
    /// to write and the caller's value stands.
    #[kithara::test(native, flash(false))]
    fn an_absent_key_stays_unset_rather_than_defaulting() {
        let patch: EngineConfigPatch =
            serde_yaml_ng::from_str("max_slots: 8\n").expect("the document types");

        assert_eq!(patch.max_slots, Some(8));
        assert!(
            patch.sample_rate.is_none(),
            "an unnamed key must stay `None` so the merge skips it"
        );
    }

    /// `eq_layout` is a real field on [`EngineConfig`] but must not be
    /// document-reachable: every construction site derives it from
    /// `generate_log_spaced_bands`, and a custom layout is installed at
    /// runtime through `PlayerImpl::set_eq_layout` (see the field's doc
    /// comment).
    #[kithara::test(native, flash(false))]
    fn the_generator_owned_eq_layout_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<EngineConfigPatch>("eq_layout: []\n")
            .expect_err("a generator-owned field must not be settable from a document");

        assert!(error.to_string().contains("eq_layout"), "{error}");
    }

    /// `channels` is a real field on [`EngineConfig`] but must not be
    /// document-reachable: its only reader is a startup log line, so a
    /// document value would change nothing the engine actually does (see the
    /// field's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_unread_channels_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<EngineConfigPatch>("channels: 1\n")
            .expect_err("a field with no consumer must not be settable from a document");

        assert!(error.to_string().contains("channels"), "{error}");
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{BeatGridId, EngineConfig};
    use crate::test_pools::{TestPools, pools};

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let config: EngineConfig<TestPools> = EngineConfig::builder()
            .grid_id(BeatGridId::allocate().expect("a grid identity"))
            .pools(pools())
            .build();

        assert_eq!(config.channels, 2);
        assert_eq!(config.sample_rate.get(), 44_100);
        assert_eq!(config.max_slots, 4);
        assert_eq!(config.eq_layout.len(), 10);
    }
}
