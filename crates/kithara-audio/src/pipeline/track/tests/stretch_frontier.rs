use std::{num::NonZeroU32, sync::atomic::Ordering};

use kithara_bufpool::PcmPool;
use kithara_decode::{PcmMeta, PcmSpec};
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

use super::rebuild::{Consts, next_test_chunk, route_signal_source_with_effects_and_pool};
use crate::{
    effects::timestretch::{StretchControls, TimeStretchProcessor},
    traits::AudioEffect,
};

fn signalsmith_effect(speed: f32, pool: &PcmPool) -> TimeStretchProcessor {
    let spec = PcmSpec::new(
        Consts::CHANNELS,
        NonZeroU32::new(Consts::SAMPLE_RATE).expect("test sample rate is non-zero"),
    );
    let controls = StretchControls::new(speed);
    controls.set_backend(StretchKind::Signalsmith);
    controls.set_keylock(true);
    TimeStretchProcessor::new(controls, spec, pool.clone())
}

async fn capture_unity_route(
    effects: Vec<Box<dyn AudioEffect>>,
    pool: PcmPool,
) -> Vec<(PcmMeta, Vec<u32>)> {
    let fixture =
        route_signal_source_with_effects_and_pool(Consts::SAMPLE_RATE, effects, pool).await;
    let host_sample_rate = fixture.host_sample_rate;
    let mut source = fixture.source;
    let mut route_recreated = false;
    let mut chunks = Vec::new();
    for _ in 0..8 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        chunks.push((
            chunk.meta,
            chunk
                .samples
                .iter()
                .map(|sample| sample.to_bits())
                .collect(),
        ));
    }
    host_sample_rate.store(Consts::ROUTE_SAMPLE_RATE, Ordering::Release);
    for _ in 0..8 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        chunks.push((
            chunk.meta,
            chunk
                .samples
                .iter()
                .map(|sample| sample.to_bits())
                .collect(),
        ));
    }
    assert!(route_recreated, "fixture must exercise route recreation");
    chunks
}

#[kithara::test(tokio)]
async fn unity_stretch_route_is_bit_identical_to_the_effect_free_path() {
    let pool = PcmPool::default();
    let baseline = capture_unity_route(Vec::new(), pool.clone()).await;
    let effect = signalsmith_effect(1.0, &pool);
    let unity = capture_unity_route(vec![Box::new(effect)], pool).await;

    assert_eq!(unity, baseline);
}
