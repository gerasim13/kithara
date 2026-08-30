use std::num::NonZero;

#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use kithara_platform::sync::Arc;
use kithara_signal::AudioSpec;
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use super::{
    Consts, chunk, dominant_bin, expected_bin, flush_serviced, render_serviced, renderer, sine,
};
use super::{StretchControls, WarpRenderer, spec};
use crate::test_pools::pools as test_pools;

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
    let mut fx = renderer(Arc::clone(&controls));
    let block = sine(4096);
    let mut out: Vec<f32> = Vec::new();
    for i in 0..24 {
        if i == 6 {
            controls.set_backend(replacement);
            fx.prepare(spec());
        }
        if let Some(c) = render_serviced(&mut fx, chunk(&block)) {
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
    let rebuilt = AudioSpec {
        sample_rate: NonZero::new(48_000).unwrap(),
        ..initial
    };
    let target_bytes = [initial, rebuilt]
        .map(|target_spec| {
            let pools = test_pools(usize::MAX);
            let controls = StretchControls::new(0.5);
            controls.set_keylock(true);
            controls.set_backend(backend);
            let target = WarpRenderer::new(controls, target_spec, pools.clone());
            assert!(target.engine.is_some());
            pools.stats().allocated_bytes
        })
        .into_iter()
        .max()
        .expect("the target matrix is non-empty");

    let pools = test_pools(target_bytes);
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = WarpRenderer::new(controls, initial, pools.clone());
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());

    fx.prepare(rebuilt);

    assert_eq!(fx.spec, rebuilt);
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn failed_target_rebuild_is_not_retried_without_a_new_revision(#[case] backend: StretchKind) {
    let pools = test_pools(0);
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = WarpRenderer::new(controls, spec(), pools.clone());
    assert!(fx.engine.is_none());

    fx.rebuild_pending = true;
    fx.prepare(spec());
    assert!(!fx.rebuild_pending);

    for _ in 0..8 {
        fx.prepare(spec());
    }
    assert!(fx.engine.is_none());
    assert!(!fx.rebuild_pending);
}
