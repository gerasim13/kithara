use std::num::{NonZeroU32, NonZeroUsize};

use kithara_abr::AbrController;
use kithara_config::Config;
use kithara_decode::GaplessMode;
use kithara_derive::Patch;
use kithara_effects::eq::{EqBandConfig, generate_log_spaced_bands};
use kithara_events::{DEFAULT_EVENT_BUS_CAPACITY, EventBus};
use kithara_platform::{
    CancelToken,
    atomic::{RelaxedAtomicBool, RelaxedAtomicF32},
    sync::Arc,
};
use kithara_warp::{BeatGridId, WarpConfig, WarpConfigPatch, WarpConfigPatchError};

use crate::{PlayWorker, consts, session::SessionBinding};

fn allocate_grid_id() -> BeatGridId {
    let Ok(id) = BeatGridId::allocate() else {
        panic!("process-wide beat-grid identity space is exhausted");
    };
    id
}

/// Crossfade window, in seconds, a player starts with. The single owner of
/// the engine-side default: every façade that exposes crossfade as initial
/// state reads it from here instead of restating a number.
pub const DEFAULT_CROSSFADE_DURATION: f32 = 1.0;

/// Playback-rate target a player starts with (1.0 = normal speed). Owned
/// here for the same reason as [`DEFAULT_CROSSFADE_DURATION`].
pub const DEFAULT_PLAYING_RATE: f32 = 1.0;

fn default_event_bus_capacity() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_EVENT_BUS_CAPACITY).unwrap_or_else(|| unreachable!())
}

/// Configuration for the player.
///
/// Holds the player's own tunables, the engine values it hands to the
/// [`EngineConfig`] it builds, and the per-call wiring a caller passes in.
/// [`PlayerConfigPatch`] is what a configuration document may say about it.
///
/// [`EngineConfig`]: crate::EngineConfig
#[derive(Patch, Config)]
#[config(debug, builder(state_mod(vis = "pub")), patch(fallible), fields(value))]
#[non_exhaustive]
#[derive_where::derive_where(Clone)]
pub struct PlayerConfig<S> {
    /// Live mute state, initially off.
    #[config(value(bool, self.muted.load()), builder(field = RelaxedAtomicBool::new(false)), patch(skip))]
    pub(super) muted: RelaxedAtomicBool,
    /// Live output volume in `0.0..=1.0`, initially one.
    #[config(value(f32, self.volume.load()), builder(field = RelaxedAtomicF32::new(1.0)), patch(skip))]
    pub(super) volume: RelaxedAtomicF32,
    /// How resources created for this player trim leading/trailing audio.
    #[config(builder(default))]
    pub gapless_mode: GaplessMode,
    /// Initial output sample rate supplied by the owning session, handed on
    /// to the engine this player builds and to the player's own sync
    /// identity. Not a document key: `HostConfig` owns the rate, a Host
    /// rejects a player whose rate disagrees with its own, and the document
    /// names it once under `host`.
    #[config(patch(skip))]
    pub sample_rate: NonZeroU32,
    /// EQ band layout handed to the engine this player builds. Not a document
    /// key: every construction site derives it from a generator, and a custom
    /// layout is installed at runtime through `PlayerImpl::set_eq_layout`.
    #[config(
        skip = "layout moves to the live equalizer owner",
        builder(default = generate_log_spaced_bands(consts::DEFAULT_EQ_BAND_COUNT)),
        patch(skip),
        debug(skip)
    )]
    pub eq_layout: Vec<EqBandConfig>,
    /// Built-in auto-advance handler. The queue overwrites this for every queue-driven
    /// player at construction, so it is not a document key.
    #[config(
        value(bool, self.auto_advance_enabled.load()),
        wrap(default = true, with = RelaxedAtomicBool::new),
        patch(skip),
        debug(skip)
    )]
    pub auto_advance_enabled: RelaxedAtomicBool,
    /// Make audio-thread reads block on a producer-ring underrun instead of
    /// zero-filling the block. Offline (faster-than-real-time) harnesses opt
    /// in so rendered output never stretches with inserted silence while the
    /// decode worker catches up. Real-time hosts must keep the default
    /// (`false`): the audio callback can never block. Not a document key:
    /// the shipped binary is a real-time host, and only the offline test
    /// harness sets this, from Rust.
    #[config(
        skip = "offline-only blocking policy is not a product control",
        builder(default),
        patch(skip),
        debug(skip)
    )]
    pub block_on_underrun: bool,
    /// Crossfade duration in seconds. Default: [`DEFAULT_CROSSFADE_DURATION`].
    #[config(
        value(f32, self.crossfade_duration.load()),
        wrap(default = DEFAULT_CROSSFADE_DURATION, with = RelaxedAtomicF32::new),
        patch(wire = f32, from = RelaxedAtomicF32::new)
    )]
    pub crossfade_duration: RelaxedAtomicF32,
    /// Default playback-rate target (1.0 = normal). Default:
    /// [`DEFAULT_PLAYING_RATE`].
    #[config(
        value(f32, self.default_rate.load()),
        wrap(default = DEFAULT_PLAYING_RATE, with = RelaxedAtomicF32::new),
        patch(wire = f32, from = RelaxedAtomicF32::new)
    )]
    pub default_rate: RelaxedAtomicF32,
    /// Capacity of each event topic when this player creates its root bus.
    /// An injected [`EventBus`] keeps its own capacity and identity.
    #[config(builder(default = default_event_bus_capacity()))]
    pub event_bus_capacity: NonZeroUsize,
    /// Secondary lead time before EOF at which the next queued item is loaded. The
    /// queue overwrites this for every queue-driven player at construction, so it is
    /// not a document key.
    #[config(
        value(f32, self.prefetch_duration.load()),
        wrap(default = consts::DEFAULT_PREFETCH_DURATION, with = RelaxedAtomicF32::new),
        patch(skip),
        debug(skip)
    )]
    pub prefetch_duration: RelaxedAtomicF32,
    /// Maximum concurrent slots of the engine this player builds.
    /// Default: 4.
    #[config(builder(default = consts::DEFAULT_MAX_SLOTS))]
    pub max_slots: usize,
    /// Stable synchronization-group identity owned by this player.
    #[config(
        skip = "player-owned synchronization identity",
        builder(default = allocate_grid_id()),
        patch(skip),
        debug(skip)
    )]
    pub(crate) grid_id: BeatGridId,
    /// Stable identity of the track grid this player publishes as its own
    /// member. Distinct from [`Self::grid_id`]: the group and the geometry it
    /// holds are two grids, and a member is found by an identity of its own.
    #[config(
        skip = "player-owned track-grid identity",
        builder(default = allocate_grid_id()),
        patch(skip),
        debug(skip)
    )]
    pub(crate) track_grid_id: BeatGridId,
    /// Shared ABR controller. When absent, the player creates its default.
    #[config(skip = "injected ABR controller", patch(skip), debug(skip))]
    pub(crate) abr: Option<Arc<AbrController>>,
    /// Optional application deadline for control-to-presented-audio response, in output frames.
    /// When Warp has no explicit quantum, a deadline selects the player's bounded default.
    #[config(get(copy))]
    pub(crate) response_budget_frames: Option<NonZeroUsize>,
    /// Root event bus for this player.
    #[config(skip = "injected event bus", patch(skip), debug(skip))]
    pub(crate) bus: Option<EventBus>,
    /// Master cancel token for this player.
    #[config(skip = "injected cancellation resource", patch(skip), debug(skip))]
    pub(crate) cancel: Option<CancelToken>,
    /// Optional pre-bound session for isolated harnesses. Production players
    /// are constructed unbound and attached exactly once by their Host.
    #[config(skip = "injected session binding", patch(skip), debug(skip))]
    pub(crate) session: Option<SessionBinding<S>>,
    /// Explicit shared playback worker. Its pools and cancellation lifetime
    /// are configured once in [`crate::PlayWorkerConfig`].
    #[config(skip = "injected playback worker", patch(skip))]
    pub(crate) worker: PlayWorker<S>,
    /// Per-deck Warp resources. A document reaches them under `player.warp:`.
    /// Every track the deck takes starts on their keylock and backend, at the
    /// deck's default rate rather than their speed; its render lane changes
    /// all three from then on.
    #[config(
        nested,
        builder(default = WarpConfig::builder().build()),
        patch(nested, fallible)
    )]
    pub(crate) warp: WarpConfig,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{NonZeroU32, PlayerConfig};
    use crate::{
        PlayWorker, PlayWorkerConfig,
        test_pools::{TestPools, pools},
    };

    pub(super) fn config() -> PlayerConfig<TestPools> {
        PlayerConfig::builder()
            .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
            .sample_rate(NonZeroU32::new(44_100).expect("44100 is not zero"))
            .build()
    }

    #[kithara::test]
    fn defaults_match_the_documented_values() {
        let config = config();

        assert!(!config.block_on_underrun);
        assert!(config.auto_advance_enabled.load());
        assert!((config.crossfade_duration.load() - 1.0).abs() < f32::EPSILON);
        assert!((config.default_rate.load() - 1.0).abs() < f32::EPSILON);
        assert_eq!(config.event_bus_capacity.get(), 1024);
        assert!((config.prefetch_duration.load() - 3.5).abs() < f32::EPSILON);
        assert_eq!(config.max_slots, 4);
    }

    #[kithara::test]
    fn debug_output_excludes_injected_fields() {
        let config = config();
        let rendered = format!("{config:#?}");

        assert!(!rendered.contains("    grid_id:"));
        assert!(!rendered.contains("    cancel:"));
        assert!(!rendered.contains("    abr:"));
    }

    #[kithara::test(native)]
    fn default_response_budget_leaves_the_deadline_to_the_application() {
        let config = config();

        assert_eq!(config.response_budget_frames(), None);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_tests {
    use kithara_test_utils::kithara;

    use super::{
        GaplessMode, NonZeroUsize, PlayerConfigPatch, PlayerConfigPatchError, tests::config,
    };

    #[kithara::test(native, flash(false))]
    fn zero_event_bus_capacity_is_rejected() {
        let error = serde_yaml_ng::from_str::<PlayerConfigPatch>("event_bus_capacity: 0\n")
            .expect_err("zero cannot construct NonZeroUsize");

        assert!(error.to_string().contains("event_bus_capacity"), "{error}");
    }

    /// `slot_ceiling` is not a prefix of any real field (unlike `max_slot`,
    /// which would pass this assertion vacuously because the error message
    /// lists the real `max_slots` field among the valid names).
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
        config.default_rate.store(2.5);

        config.apply(patch).expect("valid player document patch");

        assert!((config.crossfade_duration.load() - 2.0).abs() < f32::EPSILON);
        assert!(
            (config.default_rate.load() - 2.5).abs() < f32::EPSILON,
            "a silent field must keep its seeded value, not reset to default"
        );
    }

    /// `sample_rate` is a real field on [`PlayerConfig`] but must not be
    /// document-reachable: `HostConfig` owns the output rate, a Host refuses
    /// a player whose rate disagrees with its own, and the document names it
    /// once under `host` (see the field's doc comment).
    ///
    /// [`PlayerConfig`]: super::PlayerConfig
    #[kithara::test(native, flash(false))]
    fn the_host_owned_sample_rate_field_is_not_a_document_key() {
        let error = serde_yaml_ng::from_str::<PlayerConfigPatch>("sample_rate: 48000\n")
            .expect_err("a host-owned field must not be settable from a player document");

        assert!(error.to_string().contains("sample_rate"), "{error}");
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
        config.crossfade_duration.store(2.5);

        config.apply(patch).expect("valid player document patch");

        assert_eq!(config.gapless_mode, GaplessMode::Disabled);
        assert!(
            (config.crossfade_duration.load() - 2.5).abs() < f32::EPSILON,
            "a sibling field must survive the patch"
        );
    }

    #[kithara::test]
    fn rejected_warp_geometry_preserves_player_settings() {
        let mut config = config();
        let previous_slots = config.max_slots;
        let mut patch = PlayerConfigPatch {
            max_slots: Some(previous_slots + 1),
            ..PlayerConfigPatch::default()
        };
        patch.warp.backends.signalsmith.block_frames = NonZeroUsize::new(16);
        patch.warp.backends.signalsmith.interval_frames = NonZeroUsize::new(32);

        assert!(matches!(
            config.apply(patch),
            Err(PlayerConfigPatchError::Warp(_))
        ));
        assert_eq!(config.max_slots, previous_slots);
        assert_eq!(
            kithara_config::Config::values(&config.warp)
                .backends
                .signalsmith
                .block_frames,
            None
        );
    }
}
