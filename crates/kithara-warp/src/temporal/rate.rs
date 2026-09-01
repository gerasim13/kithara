#[derive(Clone, Copy, Debug, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub(crate) struct RateTarget {
    #[field(get, copy)]
    speed: f32,
    #[cfg(any(test, feature = "probe"))]
    #[field(get, copy)]
    revision: u64,
}

impl RateTarget {
    pub(super) fn pack(speed: f32, revision: u32) -> u64 {
        (u64::from(revision) << 32) | u64::from(speed.to_bits())
    }

    pub(super) fn revision_from(packed: u64) -> u32 {
        let [a, b, c, d, _, _, _, _] = packed.to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }

    pub(super) fn unpack(packed: u64) -> Self {
        let [_, _, _, _, e, f, g, h] = packed.to_be_bytes();
        Self {
            speed: f32::from_bits(u32::from_be_bytes([e, f, g, h])),
            #[cfg(any(test, feature = "probe"))]
            revision: u64::from(Self::revision_from(packed)),
        }
    }
}
