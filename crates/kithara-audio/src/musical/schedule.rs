use kithara_events::PlaybackDirection;
use num_traits::ToPrimitive;

use super::{CoordinateError, SourceFrame, TrackBeat, TrackBeatMap};

/// A deck's own output frame cannot be resolved to a source coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScheduleError {
    /// The tempo or output rate does not define a finite beat advance per frame.
    #[error("beat advance per output frame must be finite and non-zero")]
    InvalidBeatAdvance,
    /// The composed track-beat coordinate is not representable.
    #[error("track beat at output frame {output_frame} is not representable")]
    TrackBeat {
        output_frame: u64,
        #[source]
        source: CoordinateError,
    },
    /// The resolved track beat lies outside the analysed marker domain.
    #[error("output frame {output_frame} resolves outside the analysed beat map")]
    OutsideMap { output_frame: u64 },
}

/// One deck's binding projected onto that deck's own output frames.
///
/// A binding relates session beats to track beats and a beat map relates track
/// beats to source frames. Under a fixed tempo both relations compose into a
/// straight line on the beat axis, so the projection carries exactly the track
/// beat at output frame zero and the signed beat advance per output frame. The
/// analysed map stays the only owner of beat-to-source geometry, which is what
/// keeps a drifting grid following its local slope instead of an average tempo.
#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct SourceSchedule {
    /// Returns the analysed map this schedule reads.
    map: TrackBeatMap,
    /// Returns the track beat aligned with output frame zero.
    #[field(get, copy)]
    origin: TrackBeat,
    /// Returns the signed track-beat advance per output frame.
    #[field(get, copy)]
    beats_per_frame: f64,
}

impl SourceSchedule {
    /// Projects a binding onto a deck rendering `output_rate` frames per second
    /// at `beats_per_second`, with `origin` aligned to output frame zero.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleError::InvalidBeatAdvance`] when the tempo and rate do
    /// not define a finite, non-zero advance.
    pub fn new(
        map: TrackBeatMap,
        origin: TrackBeat,
        beats_per_second: f64,
        output_rate: u32,
        direction: PlaybackDirection,
    ) -> Result<Self, ScheduleError> {
        let rate = output_rate
            .to_f64()
            .filter(|rate| *rate > 0.0)
            .ok_or(ScheduleError::InvalidBeatAdvance)?;
        let magnitude = beats_per_second / rate;
        if !magnitude.is_finite() || magnitude <= 0.0 {
            return Err(ScheduleError::InvalidBeatAdvance);
        }
        let beats_per_frame = match direction {
            PlaybackDirection::Forward => magnitude,
            PlaybackDirection::Reverse => -magnitude,
        };
        Ok(Self {
            map,
            origin,
            beats_per_frame,
        })
    }

    /// Resolves the continuous source coordinate due at `output_frame`.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleError`] when the composed beat is not representable or
    /// falls outside the analysed marker domain.
    pub fn source_at(&self, output_frame: u64) -> Result<SourceFrame, ScheduleError> {
        let frame = output_frame
            .to_f64()
            .ok_or(ScheduleError::InvalidBeatAdvance)?;
        let beat = TrackBeat::new(f64::from(self.origin) + frame * self.beats_per_frame).map_err(
            |source| ScheduleError::TrackBeat {
                output_frame,
                source,
            },
        )?;
        self.map
            .source_frame_at(beat)
            .ok_or(ScheduleError::OutsideMap { output_frame })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::kithara;

    use super::{PlaybackDirection, ScheduleError, SourceSchedule, TrackBeat, TrackBeatMap};
    use crate::{analysis::TrackAnalysis, waveform::BeatGrid};

    struct Consts;

    impl Consts {
        /// Host rate the source-frame axis is expressed in.
        const RATE: u32 = 48_000;
        /// Frames between markers of the even 120 BPM fixture.
        const BEAT_FRAMES: u64 = 24_000;
        /// Session tempo used by the landing oracle.
        const SESSION_BPM: f64 = 128.0;
        /// Output frames per session beat at `SESSION_BPM`: 48000 * 60 / 128.
        const OUTPUT_PER_BEAT: u64 = 22_500;
    }

    fn rate() -> NonZeroU32 {
        NonZeroU32::new(Consts::RATE).expect("invariant: fixture rate is non-zero")
    }

    fn map_from(markers: Vec<u64>) -> TrackBeatMap {
        let source_frames = markers.last().copied().unwrap_or_default() + Consts::BEAT_FRAMES;
        let analysis = TrackAnalysis::with_source_rate(
            Some(BeatGrid::new(120.0, markers, vec![0], Vec::new())),
            None,
            source_frames,
            rate(),
        );
        TrackBeatMap::new(&analysis, rate()).expect("invariant: fixture markers form a map")
    }

    /// Markers every half second: an even 120 BPM grid.
    fn even_map(beats: u64) -> TrackBeatMap {
        map_from((0..beats).map(|k| k * Consts::BEAT_FRAMES).collect())
    }

    fn schedule(map: TrackBeatMap, session_bpm: f64) -> SourceSchedule {
        SourceSchedule::new(
            map,
            TrackBeat::new(0.0).expect("invariant: zero is a finite beat"),
            session_bpm / 60.0,
            Consts::RATE,
            PlaybackDirection::Forward,
        )
        .expect("invariant: the fixture tempo defines a beat advance")
    }

    /// Tier A: a marker is due at a stamped output frame and the schedule must
    /// name its source frame exactly, with no accumulated rounding.
    #[kithara::test]
    fn markers_land_on_stamped_output_frames_with_zero_error() {
        let schedule = schedule(even_map(9), Consts::SESSION_BPM);

        for marker in 0..8_u64 {
            let due = marker * Consts::OUTPUT_PER_BEAT;
            let expected = marker * Consts::BEAT_FRAMES;

            let landed = schedule
                .source_at(due)
                .expect("invariant: the marker is inside the analysed domain");

            assert_eq!(
                f64::from(landed),
                expected as f64,
                "marker {marker} must land on its own source frame"
            );
        }
    }

    /// A grid that drifts must be followed by the slope of the segment the
    /// playhead is in, not by the track's average tempo.
    #[kithara::test]
    fn drifting_grid_follows_the_local_slope() {
        // Four beats at 118 BPM, then four at 122: the marker spacing changes
        // mid-track, so an average-tempo reading and a local reading disagree.
        let slow = (Consts::RATE as u64 * 60) / 118;
        let fast = (Consts::RATE as u64 * 60) / 122;
        let mut markers = vec![0_u64];
        for _ in 0..4 {
            let last = *markers.last().expect("invariant: seeded with one marker");
            markers.push(last + slow);
        }
        for _ in 0..4 {
            let last = *markers.last().expect("invariant: seeded with one marker");
            markers.push(last + fast);
        }
        let schedule = schedule(map_from(markers), 120.0);
        let per_beat = (Consts::RATE as u64 * 60) / 120;

        let early = advance_over_one_beat(&schedule, 0, per_beat);
        let late = advance_over_one_beat(&schedule, per_beat * 6, per_beat);

        assert!(
            (early - slow as f64).abs() < 1.0,
            "the first segment must advance at its own spacing, got {early}"
        );
        assert!(
            (late - fast as f64).abs() < 1.0,
            "the later segment must advance at its own spacing, got {late}"
        );
    }

    fn advance_over_one_beat(schedule: &SourceSchedule, from: u64, per_beat: u64) -> f64 {
        let start = schedule
            .source_at(from)
            .expect("invariant: inside the analysed domain");
        let end = schedule
            .source_at(from + per_beat)
            .expect("invariant: inside the analysed domain");
        f64::from(end) - f64::from(start)
    }

    /// Past the last marker the schedule has no answer, and says so rather
    /// than extrapolating one.
    #[kithara::test]
    fn output_past_the_analysed_domain_is_typed() {
        let schedule = schedule(even_map(4), Consts::SESSION_BPM);
        let past_end = Consts::OUTPUT_PER_BEAT * 100;

        assert_eq!(
            schedule.source_at(past_end),
            Err(ScheduleError::OutsideMap {
                output_frame: past_end
            })
        );
    }

    /// A reverse binding walks the source backwards from the same origin.
    #[kithara::test]
    fn reverse_binding_walks_the_source_backwards() {
        let map = even_map(9);
        let forward = schedule(map.clone(), Consts::SESSION_BPM);
        let reverse = SourceSchedule::new(
            map,
            TrackBeat::new(4.0).expect("invariant: four is a finite beat"),
            Consts::SESSION_BPM / 60.0,
            Consts::RATE,
            PlaybackDirection::Reverse,
        )
        .expect("invariant: the fixture tempo defines a beat advance");

        let step = Consts::OUTPUT_PER_BEAT;
        let ahead = f64::from(
            forward
                .source_at(step)
                .expect("invariant: inside the analysed domain"),
        );
        let behind = f64::from(
            reverse
                .source_at(step)
                .expect("invariant: inside the analysed domain"),
        );

        assert!(ahead > 0.0, "forward must advance from the origin");
        assert_eq!(behind, 3.0 * Consts::BEAT_FRAMES as f64);
    }
}
