use kithara_test_macros as kithara;
use num_traits::ToPrimitive;

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::run_ramp((0u64..445_410).map(|n| ((n.to_f32().expect("fixture frame fits f32")) / 1000.0).sin()).collect())]
#[case::fragments((0u16..24).map(f32::from).collect())]
#[case::sine_440(stereo(88_200, |n| { let step = std::f32::consts::TAU * 440.0 / 44_100.0; 0.5 * (step * n.to_f32().expect("fixture frame fits f32")).sin() }))]
#[case::sine_220(stereo(132_300, |n| { let step = std::f32::consts::TAU * 220.0 / 44_100.0; 0.5 * (step * n.to_f32().expect("fixture frame fits f32")).sin() }))]
#[case::nn_tone((0u32..44_100).map(|n| { let step = std::f32::consts::TAU * 220.0 / 22_050.0; 0.5 * (step * n.to_f32().expect("fixture frame fits f32")).sin() }).collect())]
#[case::step(stereo(88_200, |n| if n < 44_100 { 0.0 } else { 0.5 }))]
#[case::cancelling((0..44_100).flat_map(|_| [0.8_f32, -0.8]).collect())]
#[case::quarter_4096(vec![0.25_f32; 4096 * 2])]
#[case::quarter_10000(vec![0.25_f32; 10000 * 2])]
#[case::quarter_44100(vec![0.25_f32; 44100 * 2])]
#[case::quarter_88200(vec![0.25_f32; 88200 * 2])]
#[case::quarter_132300(vec![0.25_f32; 132300 * 2])]
#[case::quarter_176400(vec![0.25_f32; 176400 * 2])]
#[case::quarter_529200(vec![0.25_f32; 529200 * 2])]
#[case::quarter_2646000(vec![0.25_f32; 2646000 * 2])]
#[case::tenth_4096(vec![0.1_f32; 4096 * 2])]
#[case::tenth_816000(vec![0.1_f32; 816000 * 2])]
#[case::archive_tone((0u64..512).flat_map(|frame| { let phase = std::f64::consts::TAU * frame.to_f64().expect("fixture frame fits f64") / 17.0; let sample = (phase.sin() * 0.5).to_f32().expect("finite fixture sample"); [sample, sample] }).collect())]
#[case::producer_stereo(vec![1.0_f32, 3.0, -2.0, 0.0])]
#[case::producer_unity(vec![1.0_f32; 4])]
#[case::producer_mono(vec![0.25_f32, -0.5, 0.75])]
#[case::fused_seam((0u32..96_000).map(|frame| (-1.365_523_678_408_751_2 + (f64::from(frame) - 47_999.0) * 0.23925).sin().to_f32().expect("finite fixture sample")).collect())]
#[case::fused_seam_stereo((0u32..48_000).flat_map(|frame| { let sample = (-1.365_523_678_408_751_2 + (f64::from(frame) - 47_999.0) * 0.23925).sin().to_f32().expect("finite fixture sample"); [sample, sample] }).collect())]
fn beat_input(samples: Vec<f32>) -> Vec<u8> {
    samples.into_iter().flat_map(f32::to_le_bytes).collect()
}

fn stereo(frames: usize, sample: impl Fn(usize) -> f32) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let value = sample(n);
            [value, value]
        })
        .collect()
}
