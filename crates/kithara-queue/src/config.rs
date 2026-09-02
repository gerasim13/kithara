use std::{fmt, num::NonZeroUsize};

use bon::Builder;
use kithara_assets::AssetStore;
use kithara_bufpool::HasPool;
use kithara_platform::CancelToken;
use kithara_play::PlayerImpl;
use struct_patch::Patch;

/// Default parallelism cap for async track loads.
pub(crate) const DEFAULT_MAX_CONCURRENT_LOADS: NonZeroUsize = match NonZeroUsize::new(3) {
    Some(n) => n,
    None => unreachable!(),
};

/// Default prefetch lead time before EOF, in seconds.
///
/// Mirrors `kithara_play::PlayerSettings::prefetch_duration` default.
pub(crate) const DEFAULT_PREFETCH_DURATION: f32 = 3.5;

/// Queue-level knobs a configuration document can override. Extracted out of
/// [`QueueConfig`] so a document reaches exactly these tunables and never the
/// per-call wiring (`player`, `store`, `cancel`) that stays on [`QueueConfig`]
/// itself.
#[derive(Clone, Debug, Builder, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "QueueSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct QueueSettings {
    /// Max concurrent background prefetch loads. Default: 3.
    #[builder(default = DEFAULT_MAX_CONCURRENT_LOADS)]
    pub max_concurrent_loads: NonZeroUsize,

    /// Whether the queue auto-starts playback once the first registered track
    /// finishes loading. A document cannot name this: the field is read only
    /// under `cfg(any(test, feature = "probe"))` (`queue/lifecycle.rs`), and
    /// `kithara-app` ships without `probe`, so a document key would configure
    /// nothing in the binary. It carries `#[patch(skip)]` for that reason, and
    /// naming it is refused rather than silently dropped.
    #[builder(default = true)]
    #[patch(skip)]
    pub should_autoplay: bool,

    /// Lead time in seconds before EOF at which the next queued track
    /// is preloaded into the audio processor. Default: 3.5. Stays `f32`
    /// seconds rather than the campaign's `humantime` duration convention:
    /// the value already reaches 10 setter and 14 read call sites as a bare
    /// `f32`, and converting the type would only churn those for a
    /// formatting preference.
    #[builder(default = DEFAULT_PREFETCH_DURATION)]
    pub prefetch_duration: f32,

    /// Entries the navigation history keeps. Only explicit selections and
    /// auto-advances land there, so the default is a listening session's
    /// worth of back-steps; the queue's own track list is unbounded.
    #[builder(default = 100)]
    pub max_history_size: usize,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for a [`Queue`](crate::Queue).
///
/// Holds queue-level defaults plus the owned [`PlayerImpl`] instance whose
/// item list the queue coordinates.
///
/// [`TrackSource::Uri`](crate::TrackSource::Uri) resources share this queue's
/// store. A caller-supplied [`ResourceConfig`](kithara_play::ResourceConfig)
/// retains its own store.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct QueueConfig<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Master cancel for the queue. `Some` threads the app master so the
    /// queue subtree cascades from one app-wide owner; `None` falls back
    /// to a fresh standalone token (test / library use). Must never be
    /// `None` on the production app path.
    pub cancel: Option<CancelToken>,

    /// Player owned and decorated by this queue.
    pub player: PlayerImpl<S>,

    /// Shared store used for bare URI track sources.
    pub store: Option<AssetStore<S>>,

    /// Queue-level knobs a configuration document can override. See
    /// [`QueueSettings`] for what a document may say.
    #[builder(default)]
    pub settings: QueueSettings,
}

impl<S> fmt::Debug for QueueConfig<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueConfig")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_play::{PlayWorker, PlayWorkerConfig, PlayerConfig};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{queue::test_session, test_pools::pools};

    #[kithara::test]
    fn default_config_has_reasonable_loader_cap() {
        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker)
                .session(test_session())
                .build(),
        );
        let cfg = QueueConfig::builder().player(player).build();
        assert_eq!(cfg.settings.max_concurrent_loads.get(), 3);
        assert!(cfg.store.is_none());
        assert!((cfg.settings.prefetch_duration - 3.5).abs() < f32::EPSILON);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_play::{PlayWorker, PlayWorkerConfig, PlayerConfig};
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{PlayerImpl, QueueConfig, QueueSettings, QueueSettingsPatch};
    use crate::{queue::test_session, test_pools::pools};

    #[kithara::test(native, flash(false))]
    fn a_document_sets_the_load_cap_and_leaves_the_history_size() {
        let patch: QueueSettingsPatch =
            serde_yaml_ng::from_str("max_concurrent_loads: 5\n").expect("the document types");
        let mut settings = QueueSettings::default();
        let history = settings.max_history_size;

        settings.apply(patch);

        assert_eq!(settings.max_concurrent_loads.get(), 5);
        assert_eq!(settings.max_history_size, history);
    }

    /// `deny_unknown_fields` arrives through `#[patch(attribute(...))]`, which
    /// emits its token stream verbatim. A typo there would generate a patch
    /// that accepts anything, and neither the compiler nor clippy would say a
    /// word -- only a bogus key proves the attribute survived generation.
    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<QueueSettingsPatch>("concurrent_load_cap: 5\n")
            .expect_err("a typo must not be silently ignored");

        assert!(error.to_string().contains("concurrent_load_cap"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn a_document_knob_reaches_the_built_config() {
        let patch: QueueSettingsPatch =
            serde_yaml_ng::from_str("max_concurrent_loads: 6\n").expect("the document types");
        let mut settings = QueueSettings::default();
        settings.apply(patch);

        let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker)
                .session(test_session())
                .build(),
        );
        let config = QueueConfig::builder()
            .player(player)
            .settings(settings)
            .build();

        assert_eq!(config.settings.max_concurrent_loads.get(), 6);
    }
}
