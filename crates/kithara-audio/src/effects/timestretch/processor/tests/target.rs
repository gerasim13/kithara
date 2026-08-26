use std::num::NonZero;

use kithara_bufpool::{ByteBudget, PcmPool};
use kithara_decode::PcmSpec;
#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use kithara_platform::sync::Arc;
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use super::{
    Consts, chunk, dominant_bin, expected_bin, flush_serviced, process_serviced, processor, sine,
};
use super::{TimeStretchProcessor, spec};
use crate::{effects::timestretch::StretchControls, traits::AudioEffect};

/// Swapping the backend mid-stream keeps the stream flowing and pitch-locked.
#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
#[kithara::test]
#[case::bungee_to_signalsmith(StretchKind::Bungee, StretchKind::Signalsmith)]
#[case::signalsmith_to_bungee(StretchKind::Signalsmith, StretchKind::Bungee)]
fn live_backend_swap_continues_and_keeps_pitch(
    #[case] initial: StretchKind,
    #[case] replacement: StretchKind,
) {
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(initial);
    let mut fx = processor(Arc::clone(&controls));
    let block = sine(4096);
    let mut out: Vec<f32> = Vec::new();
    for i in 0..24 {
        if i == 6 {
            controls.set_backend(replacement);
            fx.service_deferred(spec());
        }
        if let Some(c) = process_serviced(&mut fx, chunk(&block)) {
            out.extend_from_slice(&c.samples);
        }
    }
    while let Some(c) = flush_serviced(&mut fx) {
        out.extend_from_slice(&c.samples);
    }
    let mono: Vec<f32> = out
        .iter()
        .step_by(usize::from(Consts::CH))
        .copied()
        .collect();
    assert!(
        mono.len() >= Consts::N,
        "not enough output after swap for the FFT window"
    );
    assert!(
        dominant_bin(&mono).abs_diff(expected_bin(Consts::F0)) <= 3,
        "pitch preserved after live backend swap"
    );
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn target_rebuild_reuses_one_target_pool_budget(#[case] backend: StretchKind) {
    let initial = spec();
    let rebuilt = PcmSpec {
        sample_rate: NonZero::new(48_000).unwrap(),
        ..initial
    };
    let target_bytes = [initial, rebuilt]
        .map(|target_spec| {
            let pool = PcmPool::new(8, 0);
            let controls = StretchControls::new(0.5);
            controls.set_keylock(true);
            controls.set_backend(backend);
            let target = TimeStretchProcessor::new(controls, target_spec, pool.clone());
            assert!(target.engine.is_some());
            pool.stats().allocated_bytes
        })
        .into_iter()
        .max()
        .expect("the target matrix is non-empty");

    let pool = PcmPool::with_byte_budget(8, 0, ByteBudget(target_bytes));
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = TimeStretchProcessor::new(controls, initial, pool.clone());
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());

    let overshoots = pool.stats().budget_overshoots;
    fx.service_deferred(rebuilt);

    assert_eq!(fx.spec, rebuilt);
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());
    assert_eq!(pool.stats().budget_overshoots, overshoots);
}
