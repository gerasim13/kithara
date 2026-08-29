use std::num::NonZero;

use kithara_bufpool::{ByteBudget, SamplePool};
#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use kithara_platform::sync::Arc;
use kithara_signal::AudioSpec;
use kithara_stretch::StretchKind;
use kithara_test_utils::kithara;

use super::{StretchControls, WarpRenderer, spec};
#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use super::{chunk, render_serviced, renderer, sine};

/// Backend selection changes only at an explicit renderer lifecycle boundary.
#[cfg(all(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
#[kithara::test]
#[case::bungee_to_signalsmith_active(StretchKind::Bungee, StretchKind::Signalsmith, 0.5)]
#[case::signalsmith_to_bungee_active(StretchKind::Signalsmith, StretchKind::Bungee, 0.5)]
#[case::bungee_to_signalsmith_unity(StretchKind::Bungee, StretchKind::Signalsmith, 1.0)]
#[case::signalsmith_to_bungee_unity(StretchKind::Signalsmith, StretchKind::Bungee, 1.0)]
fn backend_change_waits_for_reset(
    #[case] initial: StretchKind,
    #[case] replacement: StretchKind,
    #[case] swap_speed: f32,
) {
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(initial);
    let mut fx = renderer(Arc::clone(&controls));
    let block = sine(4096);
    let _ = render_serviced(&mut fx, chunk(&block));
    controls.set_speed(swap_speed);
    let _ = render_serviced(&mut fx, chunk(&block));
    let admitted = fx.source_frames_admitted;

    controls.set_backend(replacement);
    fx.prepare(spec());

    assert_eq!(fx.current_kind, initial);
    assert!(fx.active);
    assert_eq!(fx.source_frames_admitted, admitted);

    fx.reset();
    fx.prepare(spec());
    assert_eq!(fx.current_kind, replacement);
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
            let sample_pool = SamplePool::new(8, 0);
            let controls = StretchControls::new(0.5);
            controls.set_keylock(true);
            controls.set_backend(backend);
            let target = WarpRenderer::new(controls, target_spec, sample_pool.clone());
            assert!(target.engine.is_some());
            sample_pool.stats().allocated_bytes
        })
        .into_iter()
        .max()
        .expect("the target matrix is non-empty");

    let sample_pool = SamplePool::with_byte_budget(8, 0, ByteBudget(target_bytes));
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = WarpRenderer::new(controls, initial, sample_pool.clone());
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());

    let overshoots = sample_pool.stats().budget_overshoots;
    fx.prepare(rebuilt);

    assert_eq!(fx.spec, rebuilt);
    assert!(fx.engine.is_some());
    assert!(fx.pending_source.is_some());
    assert!(fx.scratch.is_some());
    assert_eq!(sample_pool.stats().budget_overshoots, overshoots);
}

#[kithara::test]
#[cfg_attr(
    feature = "stretch-signalsmith",
    case::signalsmith(StretchKind::Signalsmith)
)]
#[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
fn failed_target_rebuild_is_not_retried_without_a_new_revision(#[case] backend: StretchKind) {
    let sample_pool = SamplePool::with_byte_budget(8, 0, ByteBudget(0));
    let controls = StretchControls::new(0.5);
    controls.set_keylock(true);
    controls.set_backend(backend);
    let mut fx = WarpRenderer::new(controls, spec(), sample_pool.clone());
    assert!(fx.engine.is_none());

    let initial_stats = sample_pool.stats();
    fx.rebuild_pending = true;
    fx.prepare(spec());
    let rebuild_stats = sample_pool.stats();
    assert_ne!(rebuild_stats, initial_stats);
    assert!(!fx.rebuild_pending);

    for _ in 0..8 {
        fx.prepare(spec());
    }
    assert_eq!(
        sample_pool.stats(),
        rebuild_stats,
        "a persistent preparation failure consumes one rebuild intent"
    );
}
