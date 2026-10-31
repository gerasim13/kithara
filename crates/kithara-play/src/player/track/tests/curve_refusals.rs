use kithara_audio::{Fetch, TrackStep};
use kithara_effects::{EffectConfig, EffectParameter, GainDb, eq::EqBandConfig};
use kithara_render::bridge::{EffectProgram, ParameterKnot};

use super::{
    curves::{linear_program, playing, produce, render},
    effects::{
        FRAMES, configured_track, configured_track_with, confirmed_gain, effect_source_with,
    },
    *,
};
use crate::CurveShape;

#[kithara::test]
async fn refused_effect_programs_preserve_an_already_running_curve() {
    let region = pools();
    let mut rig = rig();
    let (mut track, inbox) = configured_track(&mut rig).await;
    let (other, _) = loaded(&mut rig, B, "stale curve identity").await;
    let lane = track.lane_id.unwrap();
    let origin = (1_i64 << 40) + 4096;
    playing(&mut track, origin);
    let room = track.lane.as_ref().unwrap().available();
    let parameter = EffectParameter::EqGain { effect: 0, band: 0 };
    assert!(
        EffectProgram::new(
            parameter,
            (0..=64)
                .map(|at| ParameterKnot::new(frame(origin + at), 0.0, CurveShape::HOLD).unwrap())
                .collect()
        )
        .is_err()
    );
    assert!(
        EffectProgram::new(
            parameter,
            vec![
                ParameterKnot::new(frame(origin + 17), 0.0, CurveShape::HOLD).unwrap(),
                ParameterKnot::new(frame(origin + 17), 6.0, CurveShape::HOLD).unwrap(),
            ]
        )
        .is_err()
    );
    for value in [-24.01, 6.01] {
        assert!(
            EffectProgram::new(
                parameter,
                vec![ParameterKnot::new(frame(origin + 17), value, CurveShape::HOLD).unwrap()]
            )
            .is_err()
        );
    }
    command_at_zero(
        &mut track,
        &mut rig,
        TrackCommand::EffectProgram {
            lane,
            program: linear_program(origin, 0, 7, 87, 0.0, -12.0),
        },
    )
    .unwrap();
    let mut source = effect_source_with(inbox, &region, 13, 1);
    let mut prefix = Vec::new();
    for _ in 0..16 {
        if source.cursor().frame >= 30 {
            break;
        }
        match produce(&mut source) {
            TrackStep::Produced(Fetch::Data { data, .. }) => {
                prefix.extend_from_slice(&data.samples);
                source.admitted();
            }
            TrackStep::StateChanged => {}
            _ => panic!("curve prefix must be produced"),
        }
    }
    assert!(source.cursor().frame >= 30);
    let available = track.lane.as_ref().unwrap().available();
    for (bound, effect, invalid_target) in [(other.lane_id.unwrap(), 0, false), (lane, 9, true)] {
        let result = in_pass(&mut rig, frame(0), |out| {
            track.apply(
                TrackCommand::EffectProgram {
                    lane: bound,
                    program: linear_program(origin, effect, 47, 79, 0.0, 6.0),
                },
                out,
            )
        });
        if invalid_target {
            assert!(matches!(
                result,
                Err(PlayError::InvalidConfiguration { .. })
            ));
        } else {
            assert!(matches!(result, Err(PlayError::NotReady)));
        }
    }
    let late = in_pass(&mut rig, frame(origin), |out| {
        track.apply(
            TrackCommand::EffectProgram {
                lane,
                program: linear_program(origin, 0, -1, 17, 0.0, 6.0),
            },
            out,
        )
    });
    assert!(matches!(late, Err(PlayError::Late)));
    assert_eq!(track.lane.as_ref().unwrap().available(), available);
    let late = track
        .send_lane(
            LaneCommand::EffectProgram {
                program: linear_program(origin, 0, 1, 17, 0.0, 6.0),
                applied: None,
            },
            When::At(LaneFrame {
                segment: SegmentId::FIRST,
                frame: 1,
            }),
        )
        .unwrap();
    let (rest, _) = render(&mut track, &mut rig, &mut source);
    prefix.extend(rest);
    assert_eq!(prefix.len(), FRAMES as usize * 2);
    for (at, channels) in prefix.as_chunks::<2>().0.iter().enumerate() {
        let db = if at < 7 {
            0.0
        } else if at >= 87 {
            -12.0
        } else {
            -12.0 * (f64::from(u32::try_from(at - 7).unwrap()) / 80.0)
        };
        let db: f32 = num_traits::cast(db).unwrap();
        assert_eq!(
            *channels,
            [0.25 * 10.0_f32.powf(db / 20.0); 2],
            "frame {at}"
        );
    }
    assert_eq!(
        track
            .lane_commands
            .iter()
            .find(|command| command.seq == late)
            .unwrap()
            .applied(),
        Some(false)
    );
    assert_eq!(confirmed_gain(&track), GainDb::from(-12.0));
    assert_eq!(track.lane.as_ref().unwrap().available(), room);
}

#[kithara::test]
async fn effect_program_capacity_refuses_only_the_excess_field_and_returns_storage() {
    let region = pools();
    let mut rig = rig();
    let count = EffectProgram::MAX_ACTIVE + 1;
    let (mut track, inbox) = configured_track_with(
        &mut rig,
        vec![EffectConfig::Equalizer(vec![EqBandConfig::default()]); count],
    )
    .await;
    let origin = (1_i64 << 40) + 4096;
    playing(&mut track, origin);
    let lane = track.lane_id.unwrap();
    let room = track.lane.as_ref().unwrap().available();
    let mut sequences = Vec::new();
    for effect in 0..count {
        sequences.push(
            command_at_zero(
                &mut track,
                &mut rig,
                TrackCommand::EffectProgram {
                    lane,
                    program: linear_program(origin, effect, 7, 87, 0.0, 0.0),
                },
            )
            .unwrap(),
        );
    }
    let mut source = effect_source_with(inbox, &region, 128, count);
    let (pcm, spans) = render(&mut track, &mut rig, &mut source);
    assert_eq!(pcm, vec![0.25; FRAMES as usize * 2]);
    assert_eq!(spans, [(0, 7), (7, 80), (87, 9)]);
    for (index, seq) in sequences.into_iter().enumerate() {
        assert_eq!(
            track
                .lane_commands
                .iter()
                .find(|command| command.seq == seq)
                .unwrap()
                .applied(),
            Some(index < EffectProgram::MAX_ACTIVE)
        );
    }
    assert_eq!(track.lane.as_ref().unwrap().available(), room);
}
