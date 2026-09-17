use kithara::platform::sync::Arc;

use crate::{asset::FfiAssetStore, types::FfiKeyOptions};

/// FFI-friendly player configuration.
///
/// Carries the player's whole initial state: every field is applied while
/// [`crate::player::AudioPlayer::new`] constructs the engine through the
/// same runtime setters (`setup_hls_aes_with_rule`, `setup_network`,
/// `set_crossfade_duration`, `set_playing_rate`), so a caller never has to follow the
/// constructor with a setup call to reach the state it wanted from the start.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct FfiPlayerConfig {
    /// Shared asset store used by every item created by this player.
    pub store: Arc<FfiAssetStore>,
    /// DRM key handling — the only place key rules are declared. Pass an
    /// empty [`FfiKeyOptions`] when no DRM is needed.
    pub key_options: FfiKeyOptions,
    /// Number of EQ bands (log-spaced). Default: 10.
    pub eq_band_count: u32,
    /// Player-wide auth token written to
    /// [`crate::observer::AUTH_TOKEN_HEADER`] and merged into every item's
    /// HTTP headers. Empty means no token; change it later through
    /// [`crate::player::AudioPlayer::setup_network`].
    pub auth_token: String,
    /// Initial crossfade window in seconds. Callers that have no opinion
    /// pass [`default_crossfade_duration`]; change it later through
    /// [`crate::player::AudioPlayer::set_crossfade_duration`].
    pub crossfade_duration: f32,
    /// Initial playback-rate target (1.0 = normal). Callers that have no
    /// opinion pass [`default_playing_rate`]; change it later through
    /// [`crate::player::AudioPlayer::set_playing_rate`].
    pub playing_rate: f32,
}

/// Playback rate a player starts with when the caller has no opinion.
/// Re-exports the engine-owned [`kithara::play::DEFAULT_PLAYING_RATE`] for
/// the same reason as [`default_crossfade_duration`].
#[must_use]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn default_playing_rate() -> f32 {
    kithara::play::DEFAULT_PLAYING_RATE
}

/// Crossfade window a player starts with when the caller has no opinion.
/// Re-exports the engine-owned [`kithara::play::DEFAULT_CROSSFADE_DURATION`]
/// so Swift and Kotlin can default their own configuration to it instead of
/// restating the number.
#[must_use]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn default_crossfade_duration() -> f32 {
    kithara::play::DEFAULT_CROSSFADE_DURATION
}

#[cfg(test)]
impl FfiPlayerConfig {
    pub(crate) fn for_test() -> Self {
        Self {
            eq_band_count: 10,
            key_options: FfiKeyOptions::default(),
            store: Arc::new(FfiAssetStore::for_test()),
            auth_token: String::new(),
            crossfade_duration: kithara::play::DEFAULT_CROSSFADE_DURATION,
            playing_rate: kithara::play::DEFAULT_PLAYING_RATE,
        }
    }
}
