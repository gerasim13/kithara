#[cfg(not(feature = "usdt"))]
mod noop;
#[cfg(feature = "usdt")]
mod real;

use kithara_platform::time::Duration;
#[cfg(not(feature = "usdt"))]
pub use noop::*;
#[cfg(feature = "usdt")]
pub use real::*;

pub trait Probe {
    fn record_probe(&self, name: &'static str, operation: u64);
}

#[must_use]
pub const fn operation_id(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

pub trait IntoProbeArg: Copy {
    #[must_use]
    fn from_probe_arg(packed: u64) -> Self {
        let _ = packed;
        unimplemented!(
            "{} did not implement IntoProbeArg::from_probe_arg — override the trait method on the type whose packed `u64` you are trying to decode (or the test reads the wrong field)",
            std::any::type_name::<Self>(),
        )
    }

    fn into_probe_arg(self) -> u64;
}

macro_rules! impl_int_probe_arg {
    ($($ty:ty),* $(,)?) => {
        $(
            impl IntoProbeArg for $ty {
                fn into_probe_arg(self) -> u64 {
                    num_traits::AsPrimitive::<u64>::as_(self)
                }

                fn from_probe_arg(packed: u64) -> Self {
                    num_traits::AsPrimitive::<Self>::as_(packed)
                }
            }
        )*
    };
}

impl_int_probe_arg!(u64, i64, u32, i32, usize);

impl IntoProbeArg for bool {
    fn from_probe_arg(packed: u64) -> Self {
        packed != 0
    }

    fn into_probe_arg(self) -> u64 {
        u64::from(self)
    }
}

impl IntoProbeArg for Duration {
    fn from_probe_arg(packed: u64) -> Self {
        Self::from_micros(packed)
    }

    fn into_probe_arg(self) -> u64 {
        u64::try_from(self.as_micros()).unwrap_or(u64::MAX)
    }
}
