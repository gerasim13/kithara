#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kithara_effects::{
    AudioEffect, EffectChange, EffectTarget, GainDb,
    eq::{EqBandConfig, EqBandConfigChange, EqConfig, EqEffect},
};
use kithara_test_utils::{bufpool::pools, kithara};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[kithara::test(native)]
fn prepared_eq_steps_and_reset_preserve_targets_without_allocation() {
    let mut effect = EqEffect::new(
        &EqConfig::builder(pools()).build(),
        vec![EqBandConfig::default(); 3],
        44_100,
        2,
    )
    .unwrap();
    for band in 0..3 {
        assert_no_alloc(|| {
            effect.configure(
                EffectTarget::EqBand { effect: 0, band },
                EffectChange::EqBand(EqBandConfigChange::GainDb(GainDb::MIN)),
            )
        })
        .unwrap();
    }
    assert_no_alloc(|| effect.reset());
    for band in 0..3 {
        assert_eq!(effect.target_gain(band), Some(GainDb::MIN));
    }
    assert!(
        assert_no_alloc(|| effect.configure(
            EffectTarget::EqBand { effect: 0, band: 9 },
            EffectChange::EqBand(EqBandConfigChange::GainDb(GainDb::MAX)),
        ))
        .is_err()
    );
    assert_eq!(effect.target_gain(0), Some(GainDb::MIN));
}
