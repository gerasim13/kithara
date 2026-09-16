use std::num::NonZeroU32;

use kithara_test_utils::kithara;

use super::MasterEqNode;
use crate::{
    effects::eq::{EqConfig, generate_log_spaced_bands},
    test_pools::pools_with_budget,
};

#[kithara::test]
fn replacement_allocation_failure_is_reported_before_publication() {
    let config = EqConfig::builder(pools_with_budget(4)).build();
    let node = MasterEqNode::new(config, &generate_log_spaced_bands(10));
    assert!(
        node.layout_event(NonZeroU32::new(48_000).expect("sample rate"))
            .is_err()
    );
}
