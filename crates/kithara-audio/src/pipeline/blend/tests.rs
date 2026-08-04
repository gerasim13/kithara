use std::num::NonZeroU32;

use kithara_bufpool::PcmPool;
use kithara_decode::{BlenderProfile, PcmChunk, PcmMeta, PcmSpec};
use kithara_platform::time::Duration;
use kithara_test_utils::kithara;

use super::PcmBlender;

fn spec(channels: u16, sample_rate: u32) -> PcmSpec {
    PcmSpec::new(
        channels,
        NonZeroU32::new(sample_rate).expect("test rate must be non-zero"),
    )
}

fn chunk(spec: PcmSpec, samples: Vec<f32>) -> PcmChunk {
    let frames = samples.len() / usize::from(spec.channels);
    PcmChunk::new(
        PcmMeta {
            end_timestamp: Duration::from_millis(42),
            timestamp: Duration::from_millis(21),
            segment_index: Some(7),
            source_byte_offset: Some(1_024),
            variant_index: Some(2),
            spec,
            frames: u32::try_from(frames).expect("fixture frame count"),
            epoch: 11,
            frame_offset: 9_876,
            source_bytes: 512,
        },
        PcmPool::default().attach(samples),
    )
}

#[kithara::test]
fn single_input_blender_is_bit_exact() {
    let spec = spec(2, 48_000);
    let input = chunk(spec, vec![-1.0, -0.25, 0.0, 0.25, 0.5, 1.0]);
    let input_ptr = input.samples.as_ptr();
    let input_meta = input.meta;
    let input_bits = input
        .samples
        .iter()
        .map(|sample| sample.to_bits())
        .collect::<Vec<_>>();
    let mut blender = PcmBlender::new(BlenderProfile::new(spec));

    let output = blender.process_active(input);

    assert_eq!(output.samples.as_ptr(), input_ptr);
    assert_eq!(output.meta, input_meta);
    assert_eq!(
        output
            .samples
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        input_bits
    );
    assert!(output.samples.iter().all(|sample| sample.is_finite()));
}

#[kithara::test]
fn joining_against_an_identical_tail_is_transparent() {
    let spec = spec(2, 44_100);
    let samples = vec![-0.75, 0.5, -0.25, 0.125, 0.9, -0.4];
    let mut blender = PcmBlender::new(BlenderProfile::new(spec));

    blender.join_active(BlenderProfile::new(spec), samples.clone());
    let joined = blender.process_active(chunk(spec, samples.clone()));

    for (got, want) in joined.samples.iter().zip(samples.iter()) {
        assert!((got - want).abs() < 1.0e-6, "join shifted {want} to {got}");
    }
}

#[kithara::test]
fn the_join_opens_on_the_outgoing_tail() {
    let spec = spec(1, 44_100);
    let tail = vec![1.0, 1.0];
    let mut blender = PcmBlender::new(BlenderProfile::new(spec));

    blender.join_active(BlenderProfile::new(spec), tail);
    let joined = blender.process_active(chunk(spec, vec![-1.0, -1.0]));

    assert!((joined.samples[0] - 1.0).abs() < 1.0e-6);
    assert!(joined.samples[1].abs() < 1.0e-6);
}

#[kithara::test]
fn an_empty_tail_degenerates_to_a_hard_replace() {
    let spec = spec(2, 44_100);
    let samples = vec![0.25, -0.25, 0.5, -0.5];
    let want = samples
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let mut blender = PcmBlender::new(BlenderProfile::new(spec));

    blender.join_active(BlenderProfile::new(spec), Vec::new());
    let output = blender.process_active(chunk(spec, samples));

    assert_eq!(
        output
            .samples
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        want
    );
}

#[kithara::test]
fn replacing_active_profile_accepts_the_new_spec() {
    let initial = spec(2, 44_100);
    let replacement = spec(1, 48_000);
    let mut blender = PcmBlender::new(BlenderProfile::new(initial));
    blender.join_active(BlenderProfile::new(initial), vec![-1.0, 1.0, 0.0, 0.5]);
    let joined = blender.process_active(chunk(initial, vec![0.5, -0.5, 1.0, 0.5]));
    let expected = [-1.0, 1.0, 0.5, 0.5];
    for (got, want) in joined.samples.iter().zip(expected) {
        assert!((got - want).abs() < 1.0e-6, "join shifted {want} to {got}");
    }

    blender.join_active(BlenderProfile::new(replacement), vec![0.75, -0.75]);

    let output = blender.process_active(chunk(replacement, vec![0.25, -0.25]));

    assert_eq!(output.spec(), replacement);
    assert_eq!(
        output.samples.iter().copied().collect::<Vec<_>>(),
        [0.25, -0.25]
    );
}
