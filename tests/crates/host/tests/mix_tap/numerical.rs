use kithara_test_utils::kithara;

#[kithara::test(native, flash(false))]
fn configured_lookahead_preserves_the_fixed_numerical_contract() {
    kithara_integration_tests::limiter_numerical::check();
}
