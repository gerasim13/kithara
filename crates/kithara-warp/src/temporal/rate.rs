#[derive(Clone, Copy, Debug)]
pub(crate) struct RateTarget(u64);

impl RateTarget {
    pub(super) fn pack(speed: f32, revision: u32) -> u64 {
        (u64::from(revision) << 32) | u64::from(speed.to_bits())
    }

    pub(super) fn revision_from(packed: u64) -> u32 {
        let [a, b, c, d, _, _, _, _] = packed.to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }

    pub(super) const fn unpack(packed: u64) -> Self {
        Self(packed)
    }

    pub(crate) fn speed(self) -> f32 {
        let packed = self.0;
        let [_, _, _, _, e, f, g, h] = packed.to_be_bytes();
        f32::from_bits(u32::from_be_bytes([e, f, g, h]))
    }

    #[cfg(any(
        test,
        all(
            not(target_arch = "wasm32"),
            any(feature = "stretch-signalsmith", feature = "stretch-bungee")
        )
    ))]
    pub(crate) fn revision(self) -> u64 {
        u64::from(Self::revision_from(self.0))
    }
}
