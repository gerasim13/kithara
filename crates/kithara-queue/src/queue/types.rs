use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_platform::sync::atomic::{AtomicU64, Ordering};
pub use kithara_play::player::PlaybackView;
use kithara_play::{CrossfadeSettings, ResourceSrc, SelectionPlayback};

use crate::track::TrackSource;

/// Transition style for a track switch.
///
/// Mirrors the Apple-idiomatic pattern of a namespace-style type with
/// variants describing "what" — not "how" — so the same method
/// signature handles both manual and auto-advance cases.
///
/// - [`Transition::None`] — immediate cut (0 seconds). Matches
///   `AVQueuePlayer`'s user-initiated selection idiom.
/// - [`Transition::Crossfade`] — use the player's configured
///   [`PlayerImpl::crossfade_duration`](kithara_play::PlayerImpl::crossfade_duration).
/// - [`Transition::CrossfadeWith`] — explicit override in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Transition {
    /// No crossfade; immediate cut.
    None,
    /// Use the player's configured crossfade duration.
    Crossfade,
    /// Use an explicit crossfade duration (seconds).
    CrossfadeWith { settings: CrossfadeSettings },
}

impl Transition {
    /// Resolve the transition to an actual crossfade duration in
    /// seconds using `default` for [`Transition::Crossfade`].
    #[must_use]
    pub const fn settings(self, default: CrossfadeSettings) -> CrossfadeSettings {
        match self {
            Self::None => CrossfadeSettings {
                duration: 0.0,
                ..default
            },
            Self::Crossfade => default,
            Self::CrossfadeWith { settings } => settings,
        }
    }
}

/// A pending-select entry: a track id waiting to be applied plus the
/// [`Transition`] the caller asked for. Stored until loading finishes.
#[derive(Clone, Copy, Debug)]
pub(super) struct PendingSelect {
    pub(super) reason: crate::AdvanceReason,
    pub(super) settings: CrossfadeSettings,
    pub(super) playback: SelectionPlayback,
    pub(super) id: TrackId,
}

/// Pending-select phase. Replaces `Option<PendingSelect>` where `None`
/// conflated "idle" with "absent"; [`SelectPhase::Idle`] makes the
/// no-selection state explicit.
#[derive(Clone, Copy, Debug)]
pub(super) enum SelectPhase {
    Idle,
    Pending(PendingSelect),
}

/// Cached monotonic playback position. Replaces the `f64::NAN` sentinel
/// stored in `cached_position`; "no value yet" is the explicit
/// [`CachedPosition::Unknown`] variant.
#[derive(Clone, Copy, Debug)]
pub(super) enum CachedPosition {
    Unknown,
    Known { seconds: f64 },
}

impl CachedPosition {
    /// Build a [`CachedPosition::Known`], canonicalising a `NaN` input to
    /// [`CachedPosition::Unknown`] so the type never carries a `NaN`.
    pub(super) const fn known(seconds: f64) -> Self {
        if seconds.is_nan() {
            Self::Unknown
        } else {
            Self::Known { seconds }
        }
    }
}

impl From<CachedPosition> for Option<f64> {
    fn from(pos: CachedPosition) -> Self {
        match pos {
            CachedPosition::Known { seconds } => Some(seconds),
            CachedPosition::Unknown => None,
        }
    }
}

/// Lock-free [`CachedPosition`] cell for the `tick` hot path. The
/// `f64::NAN` bit pattern encodes [`CachedPosition::Unknown`]; any `NaN`
/// observed on load (including a `NaN` written through `store`)
/// canonicalises back to `Unknown`.
pub(super) struct AtomicCachedPosition(AtomicU64);

impl AtomicCachedPosition {
    pub(super) fn load(&self) -> CachedPosition {
        let seconds = f64::from_bits(self.0.load(Ordering::Acquire));
        if seconds.is_nan() {
            CachedPosition::Unknown
        } else {
            CachedPosition::Known { seconds }
        }
    }

    pub(super) fn store(&self, pos: CachedPosition) {
        let bits = match pos {
            CachedPosition::Unknown => f64::NAN.to_bits(),
            CachedPosition::Known { seconds } => seconds.to_bits(),
        };
        self.0.store(bits, Ordering::Release);
    }

    pub(super) fn unknown() -> Self {
        Self(AtomicU64::new(f64::NAN.to_bits()))
    }
}

/// Where a new track should land in the queue's internal `Vec`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Placement {
    /// Push past the tail — used by `Queue::append`.
    Append,
    /// Insert at a caller-resolved position — used by `Queue::insert`
    /// after it looks up `after_id`.
    At(usize),
}

/// The current track's position and duration in media seconds and the rate
/// it plays at, in media seconds per session second.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaybackTime {
    pub(crate) dur: f64,
    pub(crate) pos: f64,
    pub(crate) rate: f64,
}

impl PlaybackTime {
    /// Whether the track advances and ends within `seconds` of session time:
    /// the media time left, divided by the rate it plays at.
    pub(crate) fn ends_within(self, seconds: f32) -> bool {
        self.dur > 0.0
            && self.pos > 0.0
            && self.rate > 0.0
            && (self.dur - self.pos) / self.rate <= f64::from(seconds)
    }
}

pub(super) fn extract_track_name<S>(source: &TrackSource<S>) -> String
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    let raw = match source {
        TrackSource::Uri(s) => s.as_str(),
        TrackSource::Config(cfg) => return name_from_src(cfg.source()),
    };
    name_from_raw(raw)
}

fn name_from_src(src: &ResourceSrc) -> String {
    match src {
        ResourceSrc::Url(url) => {
            let path = url.path();
            name_from_raw(path)
        }
        ResourceSrc::Path(p) => p.file_name().map_or_else(
            || "Unknown".to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
    }
}

fn name_from_raw(s: &str) -> String {
    s.rsplit('/')
        .find(|seg| !seg.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    #[case::remaining_equals_window(157.0, 162.0, 1.0, 5.0, true)]
    #[case::remaining_below_window(160.0, 162.0, 1.0, 5.0, true)]
    #[case::far_from_end(100.0, 162.0, 1.0, 5.0, false)]
    #[case::double_speed_halves_the_session_time_left(152.0, 162.0, 2.0, 5.0, true)]
    #[case::double_speed_media_tail_is_not_yet_due(150.0, 162.0, 2.0, 5.0, false)]
    #[case::half_speed_media_tail_is_too_long(158.0, 162.0, 0.5, 5.0, false)]
    #[case::stopped_track_never_ends(161.0, 162.0, 0.0, 5.0, false)]
    #[case::zero_window_only_at_the_end(161.9, 162.0, 1.0, 0.0, false)]
    #[case::zero_position_rejected(0.0, 162.0, 1.0, 5.0, false)]
    #[case::zero_duration_rejected(10.0, 0.0, 1.0, 5.0, false)]
    fn ends_within_cases(
        #[case] pos: f64,
        #[case] dur: f64,
        #[case] rate: f64,
        #[case] window: f32,
        #[case] expected: bool,
    ) {
        assert_eq!(
            PlaybackTime { dur, pos, rate }.ends_within(window),
            expected
        );
    }

    #[kithara::test]
    fn atomic_cached_position_unknown_loads_none() {
        let cell = AtomicCachedPosition::unknown();
        assert_eq!(Option::<f64>::from(cell.load()), None);
    }

    #[kithara::test]
    fn atomic_cached_position_round_trip_zero() {
        let cell = AtomicCachedPosition::unknown();
        cell.store(CachedPosition::known(0.0));
        assert_eq!(Option::<f64>::from(cell.load()), Some(0.0));
    }

    #[kithara::test]
    fn cached_position_known_nan_canonicalises_to_unknown() {
        assert!(matches!(
            CachedPosition::known(f64::NAN),
            CachedPosition::Unknown
        ));
    }

    #[kithara::test]
    fn select_phase_pending_carries_captured_policy() {
        let phase = SelectPhase::Pending(PendingSelect {
            id: TrackId(5),
            settings: CrossfadeSettings {
                duration: 0.0,
                ..CrossfadeSettings::default()
            },
            playback: SelectionPlayback::Play,
            reason: crate::AdvanceReason::UserSelect,
        });
        match phase {
            SelectPhase::Pending(p) => {
                assert_eq!(p.id, TrackId(5));
                assert_eq!(p.settings.duration, 0.0);
                assert_eq!(p.playback, SelectionPlayback::Play);
                assert_eq!(p.reason, crate::AdvanceReason::UserSelect);
            }
            SelectPhase::Idle => panic!("expected Pending"),
        }
    }
}
