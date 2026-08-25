use kithara_warp::{
    HostBeatMap, HostEpoch, MapStamp, SessionAnchor, SessionBeat, TransportRevision,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

/// A musical tempo in beats per minute, inside the range the session clock can
/// carry.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub struct Tempo(
    /// Returns the tempo in beats per minute.
    #[field(get = beats_per_minute, copy)]
    f64,
);

impl Tempo {
    /// The slowest accepted tempo.
    pub const MIN_BEATS_PER_MINUTE: f64 = 1.0;

    /// The fastest accepted tempo. The upper bound is what keeps the anchor
    /// arithmetic finite: an unbounded tempo overflows the beat span of a
    /// single block, and the resulting failure strands the transport with an
    /// active commit and no anchor.
    pub const MAX_BEATS_PER_MINUTE: f64 = 1_000.0;

    /// Creates a tempo, rejecting non-finite and out-of-range values.
    pub fn new(beats_per_minute: f64) -> Result<Self, TempoError> {
        if (Self::MIN_BEATS_PER_MINUTE..=Self::MAX_BEATS_PER_MINUTE).contains(&beats_per_minute) {
            Ok(Self(beats_per_minute))
        } else {
            Err(TempoError { beats_per_minute })
        }
    }

    pub(crate) fn beats_per_second(self) -> f64 {
        self.0 / SECONDS_PER_MINUTE
    }
}

impl TryFrom<f64> for Tempo {
    type Error = TempoError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// The value supplied for a musical tempo was invalid.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error(
    "tempo must be between {min} and {max} beats per minute, got {beats_per_minute}",
    min = Tempo::MIN_BEATS_PER_MINUTE,
    max = Tempo::MAX_BEATS_PER_MINUTE,
)]
#[non_exhaustive]
pub struct TempoError {
    beats_per_minute: f64,
}

/// The last session transport position processed by the audio graph.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct SessionTransportSnapshot {
    /// Session-clock relation used to construct the public host-map view.
    #[field(skip)]
    anchor: SessionAnchor,
    /// Returns the host-map generation defining the live frame axis.
    #[field(get, copy)]
    host_epoch: HostEpoch,
    /// Returns the exact host-map identity and geometry revision.
    #[field(get, copy)]
    host_map_stamp: MapStamp,
    /// Returns the processed position on the session beat grid.
    #[field(get, copy)]
    position: SessionBeat,
    /// Returns the tempo that produced this processed position.
    #[field(get, copy)]
    tempo: Tempo,
    /// Returns the monotonic revision of the committed transport configuration.
    #[field(get, copy)]
    revision: TransportRevision,
    /// Returns whether the processed session transport is playing.
    #[field(get = is_playing, copy)]
    playing: bool,
}

impl SessionTransportSnapshot {
    pub(crate) const fn new(
        position: SessionBeat,
        playing: bool,
        tempo: Tempo,
        revision: TransportRevision,
        anchor: SessionAnchor,
        host_map_stamp: MapStamp,
        host_epoch: HostEpoch,
    ) -> Self {
        Self {
            anchor,
            host_epoch,
            host_map_stamp,
            position,
            tempo,
            revision,
            playing,
        }
    }

    /// Builds a read-only host map from this single atomic observation.
    ///
    /// Construction happens on the control side after reading the Copy-only
    /// transport snapshot; the audio callback never publishes or drops an
    /// allocated map handle.
    #[must_use]
    pub fn host_map(self) -> HostBeatMap {
        HostBeatMap::new(
            self.host_map_stamp.map_id(),
            self.host_map_stamp.revision(),
            self.host_epoch,
            self.anchor,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) const fn anchor(self) -> SessionAnchor {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::kithara;
    use kithara_warp::{
        BeatMapId, BeatMapRevision, HostEpoch, MapStamp, SessionAnchor, SessionBeat, SessionFrame,
    };

    use super::{SessionTransportSnapshot, Tempo, TransportRevision};

    #[kithara::test]
    fn snapshot_carries_the_anchor_that_places_a_target_on_the_session_clock() {
        let anchor = SessionAnchor::new(
            SessionFrame::new(192_000),
            SessionBeat::new(8.0).expect("invariant: fixture beat is finite"),
            2.0,
            NonZeroU32::new(48_000).expect("invariant: fixture rate is non-zero"),
        )
        .expect("invariant: fixture anchor is valid");
        let snapshot = SessionTransportSnapshot::new(
            SessionBeat::new(8.0).expect("invariant: fixture position is finite"),
            true,
            Tempo::new(120.0).expect("invariant: fixture tempo is in range"),
            TransportRevision::first(),
            anchor,
            MapStamp::new(
                BeatMapId::allocate().expect("invariant: fixture map identity space is available"),
                BeatMapRevision::first(),
            ),
            HostEpoch::new(0),
        );
        let target = SessionBeat::new(11.0).expect("invariant: fixture target is finite");

        assert_eq!(
            snapshot
                .anchor()
                .frame_at(target)
                .expect("invariant: fixture target is representable"),
            SessionFrame::new(264_000)
        );
    }

    #[kithara::test]
    fn accepts_negative_and_zero_coordinates() {
        let negative = SessionBeat::new(-1.5).expect("invariant: finite negative beat is valid");
        let zero = SessionBeat::new(0.0).expect("invariant: zero beat is valid");

        assert_eq!(f64::from(negative), -1.5);
        assert_eq!(f64::from(zero), 0.0);
    }

    #[kithara::test]
    fn rejects_non_finite_coordinates() {
        assert!(SessionBeat::new(f64::NAN).is_err());
        assert!(SessionBeat::new(f64::INFINITY).is_err());
        assert!(SessionBeat::new(f64::NEG_INFINITY).is_err());
    }
}
