use std::num::NonZeroUsize;

use kithara_config::Config;

mod consts {
    use std::num::NonZeroUsize;

    /// Batches a channel holds in flight when its config names no capacity.
    pub(super) const CAPACITY: NonZeroUsize = match NonZeroUsize::new(64) {
        Some(value) => value,
        None => unreachable!(),
    };
}

/// Sizes of one channel, fixed when it is built.
#[derive(Clone, Copy, Debug, Config)]
#[non_exhaustive]
pub struct ChannelConfig {
    /// Batches in flight at once: sent and not yet answered by a receipt.
    #[config(value, builder(default = consts::CAPACITY))]
    pub(crate) capacity: NonZeroUsize,
    /// Targets whose time batches shift; every target index is below it.
    #[config(value, builder(default))]
    pub(crate) targets: usize,
}

#[cfg(test)]
mod tests {
    use kithara_config::Config;
    use kithara_test_utils::kithara;

    use super::ChannelConfig;

    #[kithara::test]
    fn a_default_channel_reports_its_sizes_as_configuration_values() {
        let values = ChannelConfig::builder().build().values();

        assert_eq!(
            values.capacity.get(),
            64,
            "an unnamed capacity is 64 batches"
        );
        assert_eq!(values.targets, 0, "an unnamed target count is zero");
    }
}
