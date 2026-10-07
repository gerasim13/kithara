use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::{Bound, ResourceSrc};

use crate::{event::AdvanceReason, track::TrackSource};

/// A transition by press and an automatic one alike: the item it goes to and
/// the side of the frame the item enters on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub to: TrackId,
    pub bound: Bound,
}

/// The transition the queue carries out, and why.
#[derive(Clone, Copy, Debug)]
pub(super) struct Target {
    pub(super) transition: Transition,
    pub(super) reason: AdvanceReason,
    /// Scheduled ahead of the current track's end rather than pressed: the
    /// navigation cursor moves once it applies, and it gives way when nothing
    /// sounds by the time it enters.
    pub(super) auto: bool,
}

/// Cached monotonic playback position; "no value yet" is the explicit
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
    fn cached_position_known_nan_canonicalises_to_unknown() {
        assert!(matches!(
            CachedPosition::known(f64::NAN),
            CachedPosition::Unknown
        ));
    }

    #[kithara::test]
    fn a_target_carries_its_transition_and_reason() {
        let bound = Bound::AtOrAfter(kithara_signal::SessionFrame::new(64));
        let target = Target {
            transition: Transition {
                to: TrackId(5),
                bound,
            },
            reason: AdvanceReason::UserSelect,
            auto: false,
        };

        assert_eq!(target.transition.to, TrackId(5));
        assert_eq!(target.transition.bound, bound);
        assert_eq!(target.reason, AdvanceReason::UserSelect);
        assert!(!target.auto);
    }
}
