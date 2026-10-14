pub(super) use std::num::{NonZeroU16, NonZeroU32};

pub(super) use kithara_beat::{
    BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid,
};
pub(super) use kithara_platform::time::Duration as Position;
pub(super) use kithara_test_utils::kithara;

use super::{fixtures::source_grid, *};
pub(super) use crate::{Tempo, TempoStep};

#[kithara::test]
fn entry_family_uses_the_media_offset_and_the_requested_side() {
    let host = TempoTrajectory::new(
        TempoStep {
            frame: SessionFrame::new(0),
            beat: SessionBeat::default(),
            tempo: Tempo::DEFAULT,
        },
        NonZeroU16::new(4).expect("meter"),
        NonZeroU32::new(48_000).expect("rate"),
    );
    let model = BeatGridModel::try_from(RawBeatGrid {
        state: BeatGridState::Final,
        duration: Some(8.0),
        meter: Some(Meter {
            beats_per_bar: NonZeroU16::new(4).expect("meter"),
            origin_beat_ordinal: 0,
        }),
        model_id: "constant".into(),
        beats: (0..=8)
            .map(|ordinal| GridBeat {
                confidence: None,
                at: ordinal.to_f64().expect("fixture beat ordinal fits f64"),
                ordinal,
            })
            .collect(),
        downbeats: [0, 4, 8]
            .into_iter()
            .map(|beat_ordinal| GridDownbeat {
                confidence: None,
                at: beat_ordinal
                    .to_f64()
                    .expect("fixture downbeat ordinal fits f64"),
                beat_ordinal,
            })
            .collect(),
        bpm: 60.0,
        schema_version: 1,
        revision: 0,
    })
    .expect("consistent grid");
    let grid = model;
    let position = Position::from_secs_f64(0.5);
    assert_eq!(
        speed(&host, &source_grid(&grid), position, SessionFrame::new(0))
            .expect("mapped source speed"),
        2.0
    );
    assert_eq!(
        entry(
            &host,
            &source_grid(&grid),
            position,
            Bound::AtOrAfter(SessionFrame::new(20_000))
        ),
        Some(SessionFrame::new(108_000))
    );
    assert_eq!(
        entry(
            &host,
            &source_grid(&grid),
            position,
            Bound::AtOrBefore(SessionFrame::new(20_000))
        ),
        Some(SessionFrame::new(12_000))
    );
    assert_eq!(
        entry(
            &host,
            &source_grid(&grid),
            position,
            Bound::AtOrAfter(SessionFrame::new(12_000))
        ),
        Some(SessionFrame::new(12_000))
    );
    assert_eq!(
        phase_error(
            &host,
            &source_grid(&grid),
            position,
            SessionFrame::new(12_000)
        )
        .expect("mapped source phase")
        .beats,
        0.0
    );
    assert_eq!(
        jump_target(
            &host,
            &source_grid(&grid),
            position,
            SessionFrame::new(9_600)
        )
        .expect("mapped nearest phase"),
        Position::from_secs_f64(0.4)
    );
    assert!(
        entry(
            &host,
            &source_grid(&grid),
            Position::from_secs(9),
            Bound::AtOrAfter(SessionFrame::new(0))
        )
        .is_none()
    );
    let mut nonuniform = grid.as_raw().clone();
    nonuniform.meter = None;
    nonuniform.downbeats.clear();
    for beat in &mut nonuniform.beats {
        beat.at = (beat.ordinal / 2).to_f64().expect("paired beat")
            + if beat.ordinal % 2 == 0 { 0.0 } else { 0.4 };
    }
    let nonuniform = BeatGridModel::try_from(nonuniform).expect("unequal observed intervals");
    let cue = Position::from_millis(600);
    assert_eq!(
        speed(
            &host,
            &source_grid(&nonuniform),
            cue,
            SessionFrame::new(14_400)
        )
        .expect("local speed"),
        1.2,
    );
    assert_eq!(
        jump_target(
            &host,
            &source_grid(&nonuniform),
            cue,
            SessionFrame::new(14_400)
        )
        .expect("nearest source-grid phase"),
        Position::from_millis(760),
    );
    let unbounded = {
        let mut raw = nonuniform.as_raw().clone();
        raw.duration = None;
        BeatGridModel::try_from(raw).expect("observed grid without a stated end")
    };
    assert!(source_grid(&unbounded).maximum_slope().is_some());
}

#[kithara::test]
fn correction_bounds_every_step_and_cancels_the_phase_integral() {
    let epsilon = 0.001_f32;
    let model = BeatGridModel::try_from(RawBeatGrid {
        state: BeatGridState::Final,
        duration: Some(200_000.0),
        meter: None,
        model_id: "sparse-long-correction".into(),
        beats: [(0, 0.0), (400_000, 200_000.0)]
            .into_iter()
            .map(|(ordinal, at)| GridBeat {
                confidence: None,
                at,
                ordinal,
            })
            .collect(),
        downbeats: Vec::new(),
        bpm: 120.0,
        schema_version: 1,
        revision: 0,
    })
    .expect("long affine source region");
    for (from, to, seconds) in [
        (1.0, 1.0, 0.01_f64),
        (1.0, 1.1, -0.01),
        (1.2, 1.0, 0.02),
        (1.0, 1.0, 0.0),
        (1.0, 1.0, 1.01),
    ] {
        let plan = checked_correction(from, to, epsilon).expect("bounded slew");
        let mut previous = from;
        for step in &plan.steps {
            assert!((step.speed - previous).abs() <= f64::from(epsilon));
            assert!(step.seconds >= 0.0 && step.seconds.is_finite());
            previous = step.speed;
        }
        assert_eq!(previous, to);
        let host = fixtures::host(120.0 * to, 4);
        let cue = Position::from_secs_f64(1.0 + seconds.rem_euclid(0.5));
        let error = phase_error(&host, &source_grid(&model), cue, SessionFrame::new(0))
            .expect("source phase");
        let (curve, end) = corrected_speed_curve(
            &host,
            &source_grid(&model),
            cue,
            SessionFrame::new(0),
            from,
            epsilon,
            false,
        )
        .expect("source-aware correction");
        let actual = cue.as_secs_f64() + future_law::advance(&curve, 0, end) / 48_000.0;
        let geometry = source_grid(&model);
        let expected = geometry
            .media_at(
                geometry
                    .beat_at(cue.as_secs_f64())
                    .expect("initial source beat")
                    - error.shortest()
                    + 2.0 * to * end.to_f64().expect("deadline") / 48_000.0,
            )
            .expect("phase closure inside source geometry");
        assert!((actual - expected).abs() < 1e-8);
    }
}
