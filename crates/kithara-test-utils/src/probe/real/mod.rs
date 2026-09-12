#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]

/// Miri too, and not only Android: it interprets the probe call sites and
/// refuses the semaphore static each one reads (`extern static
/// __usdt_sema_kithara_probe_0 is not supported by Miri`), which is how the
/// weekly run failed in 129 seconds without reaching a single test.
#[cfg(any(target_os = "android", miri))]
#[path = "noop.rs"]
mod usdt_wire;
#[cfg(not(any(target_os = "android", miri)))]
mod usdt_wire;
mod wire;

pub use usdt_wire::{fire_0, fire_1, fire_2, fire_3, fire_4, fire_5};
pub use wire::{IntoProbeArg, Probe, operation_id, register_probes};

#[cfg(test)]
mod tests {
    use super::{fire_0, fire_1, fire_2, fire_3, fire_4, fire_5, register_probes};

    #[test]
    fn usdt_registration_and_firing_are_callable() {
        register_probes();
        register_probes();
        fire_0(0);
        fire_1(1, 1);
        fire_2(2, 1, 2);
        fire_3(3, 1, 2, 3);
        fire_4(4, 1, 2, 3, 4);
        fire_5(5, 1, 2, 3, 4, 5);
    }
}
