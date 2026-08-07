use kithara_events::PlaybackDirection;
use kithara_platform::sync::Arc;

use super::{
    CoordinateError, SessionAnchorCell, SessionBeat, SessionFrame, SourceFrame, TrackBeat,
    TrackBeatMap,
};

/// A deck's own output frame cannot be resolved to a source coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScheduleError {
    /// No transport commit has published a session grid to follow yet.
    #[error("output frame {output_frame} has no committed session grid to follow")]
    Unanchored { output_frame: u64 },
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
/// beats to source frames. The schedule owns neither relation's geometry: the
/// session side is read live from [`SessionAnchorCell`] and the source side
/// stays with the analysed map, which is what keeps a drifting grid following
/// its local slope instead of an average tempo.
///
/// What it does own is the deck's own anchor pair — the session frame and
/// session beat that output frame zero plays at. Both are fixed at bind time
/// and never recomputed: the deck's advance is measured *from where it
/// started*, so a tempo commit bends the grid ahead of the playhead and cannot
/// retroactively move a frame that has already been rendered.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct SourceSchedule {
    /// Returns the analysed map this schedule reads.
    map: TrackBeatMap,
    /// Returns the track beat aligned with output frame zero.
    #[field(get, copy)]
    origin: TrackBeat,
    /// Returns the session frame output frame zero plays at.
    #[field(get, copy)]
    start: SessionFrame,
    /// Returns the session beat playing at [`Self::start`].
    #[field(get, copy)]
    start_beat: SessionBeat,
    /// Returns the direction the source is walked in.
    #[field(get, copy)]
    direction: PlaybackDirection,
    anchor: Arc<SessionAnchorCell>,
}

impl SourceSchedule {
    /// Projects a binding onto a deck whose output frame zero plays `origin` at
    /// session frame `start`, following whatever grid `anchor` publishes.
    #[must_use]
    pub fn new(
        map: TrackBeatMap,
        origin: TrackBeat,
        start: SessionFrame,
        start_beat: SessionBeat,
        direction: PlaybackDirection,
        anchor: Arc<SessionAnchorCell>,
    ) -> Self {
        Self {
            map,
            origin,
            start,
            start_beat,
            direction,
            anchor,
        }
    }

    /// Resolves the continuous source coordinate due at `output_frame`.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleError`] when no grid is committed yet, the composed
    /// beat is not representable, or it falls outside the analysed marker
    /// domain.
    pub fn source_at(&self, output_frame: u64) -> Result<SourceFrame, ScheduleError> {
        let anchor = self
            .anchor
            .load()
            .ok_or(ScheduleError::Unanchored { output_frame })?;
        let at = self
            .start
            .offset(output_frame)
            .ok_or(ScheduleError::Unanchored { output_frame })?;
        let elapsed = anchor
            .beat_at(at)
            .map(|beat| f64::from(beat) - f64::from(self.start_beat))
            .map_err(|source| ScheduleError::TrackBeat {
                output_frame,
                source,
            })?;
        let advance = match self.direction {
            PlaybackDirection::Forward => elapsed,
            PlaybackDirection::Reverse => -elapsed,
        };
        let beat = TrackBeat::new(f64::from(self.origin) + advance).map_err(|source| {
            ScheduleError::TrackBeat {
                output_frame,
                source,
            }
        })?;
        self.map
            .source_frame_at(beat)
            .ok_or(ScheduleError::OutsideMap { output_frame })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::{
        PlaybackDirection, ScheduleError, SessionAnchorCell, SessionBeat, SessionFrame,
        SourceSchedule, TrackBeat, TrackBeatMap,
    };
    use crate::{analysis::TrackAnalysis, musical::SessionAnchor, waveform::BeatGrid};

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

    fn beat(value: f64) -> SessionBeat {
        SessionBeat::new(value).expect("invariant: the fixture beat is finite")
    }

    /// A cell already carrying one committed grid, pinned at the session
    /// origin, as the transport would publish on its first commit.
    fn committed(session_bpm: f64) -> Arc<SessionAnchorCell> {
        let cell = SessionAnchorCell::new();
        commit(&cell, SessionFrame::new(0), beat(0.0), session_bpm);
        cell
    }

    fn commit(cell: &SessionAnchorCell, frame: SessionFrame, at: SessionBeat, session_bpm: f64) {
        cell.publish(
            SessionAnchor::new(frame, at, session_bpm / 60.0, rate())
                .expect("invariant: the fixture tempo is a positive rate"),
        );
    }

    fn schedule(map: TrackBeatMap, session_bpm: f64) -> SourceSchedule {
        schedule_on(map, TrackBeat::default(), committed(session_bpm))
    }

    fn schedule_on(
        map: TrackBeatMap,
        origin: TrackBeat,
        anchor: Arc<SessionAnchorCell>,
    ) -> SourceSchedule {
        SourceSchedule::new(
            map,
            origin,
            SessionFrame::new(0),
            beat(0.0),
            PlaybackDirection::Forward,
            anchor,
        )
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
            SessionFrame::new(0),
            beat(0.0),
            PlaybackDirection::Reverse,
            committed(Consts::SESSION_BPM),
        );

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

    /// A deck with no committed grid has nowhere to put its content, and says
    /// so rather than assuming a tempo of its own.
    #[kithara::test]
    fn a_deck_with_no_committed_grid_is_typed() {
        let schedule = schedule_on(even_map(9), TrackBeat::default(), SessionAnchorCell::new());

        assert_eq!(
            schedule.source_at(0),
            Err(ScheduleError::Unanchored { output_frame: 0 })
        );
    }

    /// The defect this wave exists for: after a tempo commit the deck must
    /// advance at the new rate. A schedule holding a slope captured at bind
    /// keeps the old one and drifts away from the session for good.
    #[kithara::test]
    fn a_tempo_commit_changes_the_advance_ahead_of_the_playhead() {
        let anchor = committed(120.0);
        let schedule = schedule_on(even_map(33), TrackBeat::default(), Arc::clone(&anchor));
        let per_beat_at_120 = u64::from(Consts::RATE) * 60 / 120;
        let boundary = per_beat_at_120 * 4;
        let at_boundary = schedule
            .source_at(boundary)
            .expect("invariant: the boundary is inside the analysed domain");

        // Re-anchored at the boundary preserving the beat, as the transport
        // does: same beat, new slope.
        commit(
            &anchor,
            SessionFrame::new(i64::try_from(boundary).expect("invariant: the boundary fits")),
            beat(4.0),
            240.0,
        );

        let after = f64::from(
            schedule
                .source_at(boundary + per_beat_at_120)
                .expect("invariant: inside the analysed domain"),
        ) - f64::from(at_boundary);
        assert!(
            (after - 2.0 * Consts::BEAT_FRAMES as f64).abs() < 1.0,
            "at twice the tempo one session beat of output must consume two track beats, got {after}"
        );
    }

    /// A commit must bend the grid ahead of the playhead only. Recomputing the
    /// whole elapsed span at the new slope would move frames the deck has
    /// already rendered, which is audible as a jump.
    #[kithara::test]
    fn a_tempo_commit_does_not_move_frames_already_rendered() {
        let anchor = committed(120.0);
        let schedule = schedule_on(even_map(33), TrackBeat::default(), Arc::clone(&anchor));
        let per_beat_at_120 = u64::from(Consts::RATE) * 60 / 120;
        let boundary = per_beat_at_120 * 4;
        let before = schedule
            .source_at(boundary)
            .expect("invariant: the boundary is inside the analysed domain");

        commit(
            &anchor,
            SessionFrame::new(i64::try_from(boundary).expect("invariant: the boundary fits")),
            beat(4.0),
            240.0,
        );

        assert_eq!(
            schedule
                .source_at(boundary)
                .expect("invariant: still inside the analysed domain"),
            before,
        );
    }
}
