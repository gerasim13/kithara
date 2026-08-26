use kithara_bufpool::PcmPool;
use kithara_test_utils::kithara;

use super::build_engine;
use crate::{ElasticConfig, ElasticRequest, StretchKind};

const CHANNELS: usize = 2;
const FRAMES: usize = 4096;

fn config() -> ElasticConfig {
    ElasticConfig::builder()
        .pool(PcmPool::default())
        .sample_rate(44_100)
        .channels(CHANNELS)
        .max_source_frames(FRAMES)
        .max_output_frames(FRAMES)
        .build()
        .expect("valid factory config")
}

fn interleaved_stereo() -> Vec<f32> {
    (0..FRAMES)
        .flat_map(|frame| {
            let left = if frame % 2 == 0 { 0.25 } else { -0.25 };
            [left, -left]
        })
        .collect()
}

fn smoke(kind: StretchKind) {
    let mut engine = build_engine(kind, config()).expect("selected engine prepares");
    engine.set_pitch(1.0).expect("unity pitch is valid");
    let input = interleaved_stereo();
    let mut output = vec![f32::NAN; FRAMES * CHANNELS];
    engine
        .process(
            ElasticRequest::new(FRAMES, FRAMES).expect("unity request"),
            &input,
            &mut output,
        )
        .expect("selected engine processes an exact request");

    assert!(output.iter().all(|sample| sample.is_finite()));
}

#[cfg(feature = "stretch-signalsmith")]
#[kithara::test(native, flash(false))]
fn builds_and_processes_signalsmith_engine() {
    smoke(StretchKind::Signalsmith);
}

#[cfg(feature = "stretch-bungee")]
#[kithara::test(native, flash(false))]
fn builds_and_processes_bungee_engine() {
    smoke(StretchKind::Bungee);
}
