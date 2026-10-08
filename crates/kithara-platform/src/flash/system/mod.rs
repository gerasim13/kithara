//! [`FlashInner`] owns the virtual clock, locked scheduler core (registry and
//! scheduler), and real-I/O pacer; the lazy process instance is [`FLASH`].
//! The pacer's lazy eternal thread retains its own instance through an `Arc`.
//! Instance methods never reach for the global, so local scheduler tests and the
//! process engine behave identically. Outside this module, [`forward`] delegates
//! to the global; primitive-path tests and production use those forwards.

/// Participant credit accounting (dedicated pacers, bridged waits, blocking
/// pacer bracket and async waiter receipts) split out of the scheduler — see
/// `credit.rs`.
pub(super) mod credit;
/// Hang-dump rendering: the `fmt::Display for FlashInner` snapshot of counters,
/// quiescence pinners, parked waiters and engine primitives — see `dump.rs`.
pub(super) mod dump;
/// The process-engine facade: one thin `FLASH.method(...)` free fn per engine
/// method, the only way code outside `system/` reaches the engine — see
/// `forward.rs`.
pub(super) mod forward;
/// Per-task gate FSM ([`gate::TaskGate`]) — the waker-interception state
/// machine behind [`crate::flash::Participating`].
pub(super) mod gate;
/// The engine-state owner: `FlashInner` + `Clock` + `Core{Registry, Scheduler}`
/// + the `FLASH` process instance.
pub(super) mod inner;
/// Real-I/O pacing (the `real_io` count, pace anchor maintenance and the
/// per-instance pacer thread) — see `pace.rs` and [`crate::flash::RealIoScope`].
pub(super) mod pace;
/// Quiescence-driven virtual-clock mechanics: the advance rule plus the
/// `register_*`/`signal_*`/park surface, as `FlashInner` methods. Consumers
/// are the platform wait primitives (`thread::park_timeout`, `sync::Condvar`,
/// async `FlashSleep`/`Notify`) plus the harness.
pub(super) mod sched;
/// Per-task gate state: the [`state::TaskState`] alphabet, the typed cell
/// holding it, the park/wake outcomes, and the [`state::TaskDiag`] record the
/// gate shares with the registry (poll count and the thread that owes the next
/// poll).
pub(super) mod state;
/// Waiter wake handles ([`wake::Token`] / [`wake::Wake`]).
pub(super) mod wake;

pub(in crate::flash) use credit::AsyncHandle;
pub(in crate::flash) use forward::{
    async_acquire, describe_cvid, dump, next_condvar_id, park_timed_unparkable,
    register_channel_async, register_condvar_timed, register_condvar_untimed,
    register_notify_async, register_sleep_async, register_yield_async, signal_channel,
    signal_condvar, signal_notify, sleep_timed, unpark, yield_until_advance,
};
pub(in crate::flash) use inner::{
    Clock, Core, CvDesc, CvId, FLASH, FlashInner, Registry, SyncHolder, WaiterId,
};
pub(in crate::flash) use pace::{real_io_enter, real_io_exit};
pub(in crate::flash) use sched::ParkRole;
