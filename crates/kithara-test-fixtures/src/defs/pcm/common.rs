use kithara_test_macros as kithara;

use crate::signal::SAW_PERIOD;

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::ramp((1u16..=129).map(f32::from).collect())]
#[case::negative_ramp((1u16..=129).map(|value| -f32::from(value)).collect())]
#[case::silence(vec![0.0; 6])]
#[case::provenance_silence(vec![0.0; 64])]
#[case::phase_endpoints([i16::MIN, i16::MIN + 1, 0, i16::MAX].map(|value| f32::from(value) / 32_768.0).to_vec())]
#[case::direction_step(vec![-1.0, -1.0, -0.9999695, -0.9999695])]
#[case::direction_channel_less(vec![-1.0, -0.5])]
#[case::stereo_pair(vec![1.0, 3.0, 2.0, 6.0])]
fn pcm(samples: Vec<f32>) -> Vec<u8> {
    samples.into_iter().flat_map(f32::to_le_bytes).collect()
}

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::mono(1)]
#[case::stereo(2)]
#[case::three(3)]
#[case::four(4)]
#[case::five(5)]
#[case::six(6)]
#[case::seven(7)]
#[case::eight(8)]
#[case::nine(9)]
fn channel_labels(channels: u16) -> Vec<u8> {
    (0u16..5)
        .flat_map(|frame| (0..channels).map(move |channel| f32::from(frame * 16 + channel)))
        .flat_map(f32::to_le_bytes)
        .collect()
}

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::ascending(0, 203_000, false)]
#[case::ascending_wrap(SAW_PERIOD - 32, 128, false)]
#[case::descending(0, 64, true)]
#[case::descending_wrap(SAW_PERIOD - 32, 64, true)]
fn provenance(start: usize, len: usize, descending: bool) -> Vec<u8> {
    (start..start + len)
        .flat_map(|frame| {
            let unit = i32::try_from(frame % SAW_PERIOD).expect("phase fits i32");
            let value = if descending {
                32_767 - unit
            } else {
                unit - 32_768
            };
            let sample = i16::try_from(value).expect("phase sample fits i16");
            (f32::from(sample) / 32_768.0).to_le_bytes()
        })
        .collect()
}
