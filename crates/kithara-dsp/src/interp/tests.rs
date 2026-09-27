use kithara_test_utils::kithara;

use super::{InterpError, Interpolation, interpolate};

const METHODS: [Interpolation; 4] = [
    Interpolation::Linear,
    Interpolation::Quadratic,
    Interpolation::Hermite,
    Interpolation::Watte,
];
const UNWRITTEN: f32 = -1.0;
/// One frame past the largest window an `f32` position addresses exactly.
const OVERSIZED: usize = 16_777_217;
const SMALL: [f32; 6] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
/// No `−0.0` and no zero: at `x = 0` the formulas return `y0` bit for bit
/// only for other values.
const WINDOW: [f32; 8] = [0.1, -0.7, 0.3, 0.9, -0.2, 0.5, -0.4, 0.8];

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

#[kithara::test]
fn padding_names_the_frames_each_method_reads() {
    assert_eq!(
        METHODS.map(Interpolation::padding),
        [(0, 1), (1, 1), (1, 2), (1, 2)]
    );
}

#[kithara::test]
fn out_of_window_positions_fail_without_writing() {
    let oversized = vec![0.0_f32; OVERSIZED];
    let len = u8::try_from(SMALL.len()).expect("six frames");
    for method in METHODS {
        let (before, after) = method.padding();
        let low = f32::from(before);
        let cases: [(&[f32], f32); 4] = [
            (SMALL.as_slice(), f32::NAN),
            (SMALL.as_slice(), f32::from(len.saturating_sub(after))),
            (SMALL.as_slice(), low - 0.5),
            (oversized.as_slice(), 1.5),
        ];
        for (window, bad) in cases {
            let mut output = [UNWRITTEN; 2];
            assert_eq!(
                interpolate(method, window, &[low, bad], &mut output),
                Err(InterpError::OutOfWindow),
                "{method:?} at {bad} in {} frames",
                window.len()
            );
            assert_eq!(bits(&output), bits(&[UNWRITTEN; 2]), "{method:?} at {bad}");
        }
    }
}

#[kithara::test]
fn integer_positions_return_the_window_samples() {
    let len = u8::try_from(WINDOW.len()).expect("eight frames");
    for method in METHODS {
        let (before, after) = method.padding();
        let positions: Vec<f32> = (before..len.saturating_sub(after)).map(f32::from).collect();
        let mut output = vec![UNWRITTEN; positions.len()];
        assert_eq!(
            interpolate(method, &WINDOW, &positions, &mut output),
            Ok(positions.len()),
            "{method:?}"
        );
        let expected: Vec<f32> = WINDOW
            .iter()
            .copied()
            .skip(usize::from(before))
            .take(positions.len())
            .collect();
        assert_eq!(bits(&output), bits(&expected), "{method:?}");
    }
}
