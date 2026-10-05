use kithara_test_utils::kithara;
use num_traits::AsPrimitive;

use super::VariantIndex;
use crate::consts;

/// ABR mode selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, kithara::IntoProbeArg)]
#[probe_arg(encode_only, with = Self::encode_probe_arg)]
pub enum AbrMode {
    /// Automatic bitrate adaptation.
    /// Optional initial variant index (defaults to 0 when `None`).
    Auto(Option<VariantIndex>),
    /// Manual variant selection — ABR disabled, fixed variant.
    Manual(VariantIndex),
}

impl Default for AbrMode {
    fn default() -> Self {
        Self::Auto(None)
    }
}

impl AbrMode {
    fn encode_probe_arg(self) -> u64 {
        AsPrimitive::<u64>::as_(usize::from(self))
    }

    /// Manual mode pinned to variant `idx`. Shorthand for
    /// `Manual(VariantIndex::new(idx))` — the index is wrapped without a
    /// bounds check; validate at trust boundaries via
    /// [`VariantIndex::try_new`] / `AbrHandle::set_mode`.
    #[must_use]
    pub const fn manual(idx: usize) -> Self {
        Self::Manual(VariantIndex::new(idx))
    }
}

impl From<AbrMode> for usize {
    fn from(mode: AbrMode) -> Self {
        match mode {
            AbrMode::Manual(v) => {
                debug_assert!(
                    v.get() < consts::ABR_MODE_AUTO_THRESHOLD,
                    "variant index too large"
                );
                v.get()
            }
            AbrMode::Auto(None) => Self::MAX,
            AbrMode::Auto(Some(v)) => {
                debug_assert!(
                    v.get() < consts::ABR_MODE_AUTO_THRESHOLD,
                    "variant index too large"
                );
                Self::MAX - 1 - v.get()
            }
        }
    }
}

impl From<usize> for AbrMode {
    fn from(val: usize) -> Self {
        if val == usize::MAX {
            Self::Auto(None)
        } else if val >= consts::ABR_MODE_AUTO_THRESHOLD {
            Self::Auto(Some(VariantIndex::new(usize::MAX - 1 - val)))
        } else {
            Self::Manual(VariantIndex::new(val))
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::{kithara, probe::IntoProbeArg};

    use super::{AbrMode, VariantIndex};

    #[kithara::test]
    #[case(AbrMode::Auto(None))]
    #[case(AbrMode::Auto(Some(VariantIndex::new(0))))]
    #[case(AbrMode::Auto(Some(VariantIndex::new(5))))]
    #[case(AbrMode::Auto(Some(VariantIndex::new(42))))]
    #[case(AbrMode::Manual(VariantIndex::new(0)))]
    #[case(AbrMode::Manual(VariantIndex::new(1)))]
    #[case(AbrMode::Manual(VariantIndex::new(99)))]
    fn abr_mode_usize_round_trip(#[case] mode: AbrMode) {
        let encoded: usize = mode.into();
        let decoded: AbrMode = encoded.into();
        assert_eq!(decoded, mode);
    }

    #[kithara::test]
    fn manual_and_auto_encode_differently() {
        let manual: usize = AbrMode::Manual(VariantIndex::new(0)).into();
        let auto: usize = AbrMode::Auto(None).into();
        assert_ne!(manual, auto);
    }

    #[kithara::test]
    fn abr_mode_probe_argument_keeps_the_existing_usize_wire() {
        for mode in [
            AbrMode::Auto(None),
            AbrMode::Auto(Some(VariantIndex::new(0))),
            AbrMode::Auto(Some(VariantIndex::new(42))),
            AbrMode::Manual(VariantIndex::new(0)),
            AbrMode::Manual(VariantIndex::new(42)),
        ] {
            assert_eq!(
                mode.into_probe_arg(),
                u64::try_from(usize::from(mode)).expect("usize fits the u64 probe wire")
            );
        }
    }
}
