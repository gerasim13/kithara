use kithara_core_test_fixtures::{
    channel_signals, negative_pcm_ramp, pcm_ramp, silence_pcm, stereo_pair, trim_ramp,
};
#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;
use kithara_test_utils::kithara;

fn bits(samples: Vec<f32>) -> Vec<u32> {
    samples.into_iter().map(f32::to_bits).collect()
}

#[kithara::test]
fn prepared_pcm_preserves_every_authored_sample_bit() {
    assert_eq!(
        bits(trim_ramp()),
        bits((0_u16..4096).map(f32::from).collect())
    );
    assert_eq!(
        bits(pcm_ramp()),
        bits((1_u16..=129).map(f32::from).collect())
    );
    assert_eq!(
        bits(negative_pcm_ramp()),
        bits((1_u16..=129).map(|value| -f32::from(value)).collect())
    );
    assert_eq!(bits(silence_pcm()), vec![0; 6]);
    assert_eq!(bits(stereo_pair()), bits(vec![1.0, 3.0, 2.0, 6.0]));
    for (samples, channels) in channel_signals().into_iter().zip(1_u16..=9) {
        let authored = (0_u16..5)
            .flat_map(|frame| (0..channels).map(move |channel| f32::from(frame * 16 + channel)))
            .collect();
        assert_eq!(bits(samples), bits(authored));
    }
}
