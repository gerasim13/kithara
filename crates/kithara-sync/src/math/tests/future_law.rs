use std::num::{NonZeroU16, NonZeroU32};

use kithara_beat::BeatGridState;
use kithara_signal::{ExponentialApproach, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::{SessionBeat, SpeedCurve};
use num_traits::ToPrimitive;

use super::fixtures::{grid, host};
use crate::{CorrectionPlan, CorrectionStep, Tempo, TempoStep, TempoTrajectory, speed_curve};

fn approached(initial: f64, target: f64, frames: u64) -> f64 {
    target + (initial - target) * (-frames.to_f64().expect("fixture offset") / 240.0).exp()
}

fn analytic_laws(curve: &SpeedCurve) -> Vec<(u64, ExponentialApproach)> {
    match curve {
        SpeedCurve::Approach(law) => vec![(0, *law)],
        SpeedCurve::ApproachSteps(laws) => laws.to_vec(),
        other => panic!("expected an analytic law, received {other:?}"),
    }
}

fn rate_at(curve: &SpeedCurve, frame: u64) -> f64 {
    let laws = analytic_laws(curve);
    let (start, law) = laws
        .iter()
        .rev()
        .find(|(at, _)| *at <= frame)
        .expect("law covers the frame");
    law.rate_at((frame - start).to_f64().expect("fixture offset"))
        .expect("valid law")
}

fn advance(curve: &SpeedCurve, start: u64, end: u64) -> f64 {
    assert!(start <= end);
    let laws = analytic_laws(curve);
    laws.iter()
        .enumerate()
        .filter_map(|(index, (at, law))| {
            let after = laws.get(index + 1).map_or(end, |(next, _)| *next);
            let from = start.max(*at);
            let to = end.min(after);
            (from < to).then(|| {
                law.advance_over(
                    (from - at).to_f64().expect("fixture offset"),
                    (to - from).to_f64().expect("fixture duration"),
                )
                .expect("valid interval")
            })
        })
        .sum()
}

fn pending_targets() -> TempoTrajectory {
    let mut trajectory = host(120.0, 4);
    trajectory
        .push(SessionFrame::new(1_128), Tempo::new(90.0).expect("tempo"))
        .expect("later target");
    trajectory
        .push(SessionFrame::new(1_000), Tempo::new(180.0).expect("tempo"))
        .expect("earlier target reanchors its successor");
    trajectory
}

#[kithara::test]
fn future_targets_are_projected_in_frame_order_after_out_of_order_acceptance() {
    let trajectory = pending_targets();
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let curve =
        speed_curve(&trajectory, &model, SessionFrame::new(800)).expect("finite future trajectory");
    let laws = analytic_laws(&curve);

    assert_eq!(
        laws.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        [0, 200, 328]
    );
    assert_eq!(laws[0].1.initial(), 1.0);
    assert_eq!(laws[0].1.target(), 1.0);
    assert_eq!(laws[1].1.initial(), 1.0);
    assert_eq!(laws[1].1.target(), 1.5);
    assert_eq!(laws[1].1.time_constant(), 240.0);
    assert!((laws[2].1.initial() - approached(1.0, 1.5, 128)).abs() < 1e-12);
    assert_eq!(laws[2].1.target(), 0.75);
    assert_eq!(laws[2].1.time_constant(), 240.0);
    for frame in [0, 199, 200, 201, 327, 328, 329, 700] {
        let expected = match frame {
            0..200 => 1.0,
            200..328 => approached(1.0, 1.5, frame - 200),
            _ => approached(approached(1.0, 1.5, 128), 0.75, frame - 328),
        };
        assert!(
            (rate_at(&curve, frame) - expected).abs() < 1e-12,
            "frame {frame}"
        );
    }
    let at_target = speed_curve(&trajectory, &model, SessionFrame::new(1_000))
        .expect("projection from a command boundary");
    assert_eq!(
        analytic_laws(&at_target)
            .iter()
            .map(|(at, _)| *at)
            .collect::<Vec<_>>(),
        [0, 128]
    );
}

#[kithara::test]
fn withdrawing_an_earlier_target_removes_it_from_the_future_projection() {
    let mut trajectory = pending_targets();
    trajectory.withdraw(SessionFrame::new(1_000));
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let curve = speed_curve(&trajectory, &model, SessionFrame::new(800))
        .expect("withdrawn target is absent");
    let laws = analytic_laws(&curve);

    assert_eq!(laws.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [0, 328]);
    assert_eq!(laws[1].1.initial(), 1.0);
    assert_eq!(laws[1].1.target(), 0.75);
    assert_eq!(laws[1].1.time_constant(), 240.0);
    assert_eq!(rate_at(&curve, 200), 1.0);
    assert!((rate_at(&curve, 329) - approached(1.0, 0.75, 1)).abs() < 1e-12);
}

#[kithara::test]
#[case::positive(9_007_199_254_740_993)]
#[case::negative(-9_007_199_254_740_993)]
fn projection_keeps_exact_frame_offsets_beyond_f64_integer_precision(#[case] origin: i64) {
    let mut trajectory = TempoTrajectory::new(
        TempoStep {
            frame: SessionFrame::new(origin),
            beat: SessionBeat::default(),
            tempo: Tempo::new(120.0).expect("tempo"),
        },
        NonZeroU16::new(4).expect("meter"),
        NonZeroU32::new(48_000).expect("rate"),
    );
    trajectory
        .push(
            SessionFrame::new(origin + 259),
            Tempo::new(90.0).expect("tempo"),
        )
        .expect("later target");
    trajectory
        .push(
            SessionFrame::new(origin + 131),
            Tempo::new(180.0).expect("tempo"),
        )
        .expect("earlier target");
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let curve = speed_curve(&trajectory, &model, SessionFrame::new(origin + 7))
        .expect("integer origin projection");
    let laws = analytic_laws(&curve);

    assert_eq!(
        laws.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        [0, 124, 252]
    );
    assert_eq!(rate_at(&curve, 123), 1.0);
    assert_eq!(rate_at(&curve, 124), 1.0);
    assert!((rate_at(&curve, 125) - approached(1.0, 1.5, 1)).abs() < 1e-12);
    assert!((laws[2].1.initial() - approached(1.0, 1.5, 128)).abs() < 1e-12);
}

#[kithara::test]
fn zero_correction_preserves_the_complete_future_law() {
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let base =
        speed_curve(&pending_targets(), &model, SessionFrame::new(800)).expect("future trajectory");
    let plan = CorrectionPlan {
        steps: vec![
            CorrectionStep {
                speed: 1.0,
                seconds: 0.005,
            },
            CorrectionStep {
                speed: 1.0,
                seconds: 0.0,
            },
        ],
    };
    let (actual, end) = plan
        .checked_curve_following(NonZeroU32::new(48_000).expect("rate"), &base, 1.0)
        .expect("zero offset");

    assert_eq!(end, 240);
    assert_eq!(actual, base);
}

#[kithara::test]
fn correction_restores_an_approach_between_two_host_commands() {
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let base =
        speed_curve(&pending_targets(), &model, SessionFrame::new(800)).expect("future trajectory");
    let plan = CorrectionPlan {
        steps: vec![
            CorrectionStep {
                speed: 1.25,
                seconds: 0.005,
            },
            CorrectionStep {
                speed: 1.0,
                seconds: 0.0,
            },
        ],
    };
    let (actual, end) = plan
        .checked_curve_following(NonZeroU32::new(48_000).expect("rate"), &base, 1.0)
        .expect("correction ends during the Host approach");
    let laws = analytic_laws(&actual);

    assert_eq!(end, 240);
    assert_eq!(
        laws.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        [0, 200, 240, 328]
    );
    assert_eq!(laws[2].1.target(), 1.5);
    assert_eq!(laws[2].1.time_constant(), 240.0);
    assert!((laws[2].1.initial() - approached(1.0, 1.5, 40)).abs() < 1e-12);
    for frame in [239, 240, 241, 327, 328, 329, 700] {
        let offset = if frame < end { 0.25 } else { 0.0 };
        assert!((rate_at(&actual, frame) - rate_at(&base, frame) - offset).abs() < 1e-12);
    }
    assert!((advance(&actual, 0, 700) - advance(&base, 0, 700) - 60.0).abs() < 1e-9);
    assert!((advance(&actual, end, 700) - advance(&base, end, 700)).abs() < 1e-9);
}

#[kithara::test]
fn correction_collisions_preserve_host_targets_and_restore_the_remaining_law() {
    let mut trajectory = pending_targets();
    trajectory
        .push(SessionFrame::new(1_800), Tempo::new(150.0).expect("tempo"))
        .expect("target after correction ends");
    let model = grid(480_000, 480_000, 24_000, 0, None, BeatGridState::Final);
    let base = speed_curve(&trajectory, &model, SessionFrame::new(800)).expect("future trajectory");
    let plan = CorrectionPlan {
        steps: vec![
            CorrectionStep {
                speed: 1.25,
                seconds: 200.0 / 48_000.0,
            },
            CorrectionStep {
                speed: 0.75,
                seconds: 128.0 / 48_000.0,
            },
            CorrectionStep {
                speed: 1.0,
                seconds: 0.0,
            },
        ],
    };
    let (actual, end) = plan
        .checked_curve_following(NonZeroU32::new(48_000).expect("rate"), &base, 1.0)
        .expect("finite correction on future law");
    let laws = analytic_laws(&actual);

    assert_eq!(end, 328);
    assert_eq!(
        laws.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        [0, 200, 328, 1_000]
    );
    assert_eq!(laws[1].1.target(), 1.25);
    assert_eq!(laws[1].1.time_constant(), 240.0);
    assert_eq!(laws[2].1, analytic_laws(&base)[2].1);
    assert_eq!(laws[3].1, analytic_laws(&base)[3].1);
    for frame in [0, 1, 199, 200, 201, 327, 328, 329, 999, 1_000, 1_001, 2_000] {
        let offset = match frame {
            0..200 => 0.25,
            200..328 => -0.25,
            _ => 0.0,
        };
        assert!(
            (rate_at(&actual, frame) - rate_at(&base, frame) - offset).abs() < 1e-12,
            "frame {frame}"
        );
    }
    assert!((advance(&actual, 0, 2_000) - advance(&base, 0, 2_000) - 18.0).abs() < 1e-9);
    let cuts = [
        0, 1, 127, 199, 200, 201, 327, 328, 329, 999, 1_000, 1_001, 2_000,
    ];
    let partitioned: f64 = cuts
        .windows(2)
        .map(|part| advance(&actual, part[0], part[1]))
        .sum();
    assert!((partitioned - advance(&actual, 0, 2_000)).abs() < 1e-9);
    assert!((advance(&actual, end, 2_000) - advance(&base, end, 2_000)).abs() < 1e-9);
}
