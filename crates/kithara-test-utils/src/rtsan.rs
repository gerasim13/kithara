//! `RealtimeSanitizer` context marking and escape hatches.
//!
//! Under `--cfg rtsan` the audio worker / RT paths are checked entry points
//! (via `#[kithara::rtsan_forbid_blocking]`), so `RTSan` aborts on any
//! `malloc` / `free` / lock / syscall reached from them. A function annotated
//! `#[kithara::rtsan_allow_blocking]` opens a [`permit`] guard for its whole
//! body — used to *inventory* genuinely-unavoidable blocking points one at a
//! time while the surrounding coordination stays checked.
//!
//! Two backends answer to the same annotations, selected by `--cfg
//! rtsan_standalone` alongside `--cfg rtsan`:
//!
//! - *instrumented* (nightly): the compiler marks the entry point through
//!   `#[sanitize(realtime = "nonblocking")]` and links the runtime from
//!   `-Zsanitizer=realtime`; this module reaches the runtime's `__rtsan_disable`
//!   / `__rtsan_enable` C entry points directly.
//! - *standalone* (stable): the entry point is marked at runtime by a
//!   [`realtime_scope`] guard around the body, and the runtime comes from the
//!   `rtsan-standalone` crate, whose own entry points are no-ops unless its
//!   build saw `RTSAN_ENABLE`.
//!
//! Detection is identical either way — it is the runtime's libc interceptors
//! that report, not the compiler.
//!
//! [`permit`] is **reentrant**: a thread-local depth counter toggles the
//! runtime only at the outermost guard, so annotating both a caller and a
//! callee (or re-entering through recursion) stays correct regardless of
//! whether the runtime's own `disable` is refcounted. Off `rtsan` every guard
//! here is a zero-cost ZST.
#[cfg(all(rtsan, not(rtsan_standalone)))]
unsafe extern "C" {
    fn __rtsan_disable();
    fn __rtsan_enable();
}

#[cfg(all(rtsan, not(rtsan_standalone)))]
#[inline]
fn suspend_checks() {
    // SAFETY: `__rtsan_disable` is a no-arg C entry point from the runtime
    // linked by `-Zsanitizer=realtime`; `resume_checks` pairs with it.
    unsafe { __rtsan_disable() }
}

#[cfg(all(rtsan, not(rtsan_standalone)))]
#[inline]
fn resume_checks() {
    // SAFETY: paired with `suspend_checks`; `__rtsan_enable` is a no-arg C
    // entry point from the linked runtime.
    unsafe { __rtsan_enable() }
}

#[cfg(all(rtsan, rtsan_standalone))]
#[inline]
fn suspend_checks() {
    rtsan_standalone::disable();
}

#[cfg(all(rtsan, rtsan_standalone))]
#[inline]
fn resume_checks() {
    rtsan_standalone::enable();
}

/// Brings the runtime up before any shared library's initializer runs.
///
/// On Linux the standalone runtime is a static archive whose libc interceptors
/// are live from the first instruction, while the real `malloc` behind them is
/// resolved on first use. The loader runs the initializers of the program's
/// shared libraries before the program's own, and glib's — reached through
/// ffmpeg, ALSA and D-Bus — allocates in its. That call landed in the
/// interceptor while the real allocator was still null and jumped to address
/// zero: every lane died in the loader on `SIGSEGV`, before `main`, before the
/// runtime could install its own signal handler, with an empty stderr and no
/// test named.
///
/// `.preinit_array` is the one hook the loader runs ahead of every shared
/// library initializer, so it is the only point early enough to be safe. macOS
/// links the runtime as a dylib whose own initializer already covers this, and
/// has no equivalent section.
#[cfg(all(rtsan, rtsan_standalone, target_os = "linux"))]
#[used]
#[unsafe(link_section = ".preinit_array")]
static RTSAN_PREINIT: extern "C" fn() = initialize_before_shared_libraries;

#[cfg(all(rtsan, rtsan_standalone, target_os = "linux"))]
extern "C" fn initialize_before_shared_libraries() {
    rtsan_standalone::ensure_initialized();
}

#[cfg(rtsan)]
thread_local! {
    /// Nesting depth of live [`Permit`] guards on this thread.
    static PERMIT_DEPTH: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

/// RAII guard that re-enables `RealtimeSanitizer` checks when the outermost
/// guard on the thread drops. Created by [`permit`]; emitted by
/// `#[kithara::rtsan_allow_blocking]`.
#[cfg(rtsan)]
#[must_use = "the permit only suspends RTSan checks while the guard is alive"]
pub struct Permit;

#[cfg(rtsan)]
impl Drop for Permit {
    fn drop(&mut self) {
        PERMIT_DEPTH.with(|depth| {
            let outer = depth.get().saturating_sub(1);
            depth.set(outer);
            if outer == 0 {
                resume_checks();
            }
        });
    }
}

/// Suspend `RealtimeSanitizer` blocking-checks until the returned guard drops.
///
/// Reentrant: only the outermost live guard toggles the runtime.
#[cfg(rtsan)]
#[inline]
pub fn permit() -> Permit {
    PERMIT_DEPTH.with(|depth| {
        let prev = depth.get();
        depth.set(prev + 1);
        if prev == 0 {
            suspend_checks();
        }
    });
    Permit
}

/// Zero-cost no-op guard when `RealtimeSanitizer` is not compiled in.
#[cfg(not(rtsan))]
#[must_use]
pub struct Permit;

/// No-op when `RealtimeSanitizer` is not compiled in.
#[cfg(not(rtsan))]
#[inline(always)]
pub const fn permit() -> Permit {
    Permit
}

/// RAII guard that marks its scope a checked realtime context for the
/// standalone backend, which has no compiler attribute to carry that mark.
///
/// Emitted by `#[kithara::rtsan_forbid_blocking]` around the whole body. Under
/// the instrumented backend the mark comes from the compiler instead and this
/// guard is a zero-cost ZST, so the annotation expands to one shape for both.
#[cfg(all(rtsan, rtsan_standalone))]
#[must_use = "the scope only marks a realtime context while the guard is alive"]
pub struct RealtimeScope(rtsan_standalone::ScopedSanitizeRealtime);

/// Mark this scope a checked realtime context.
#[cfg(all(rtsan, rtsan_standalone))]
#[inline]
pub fn realtime_scope() -> RealtimeScope {
    RealtimeScope(rtsan_standalone::ScopedSanitizeRealtime::default())
}

/// Zero-cost no-op: the instrumented backend marks the context at the
/// compiler's `sanitize` attribute, and off `rtsan` nothing is checked.
#[cfg(not(all(rtsan, rtsan_standalone)))]
#[must_use]
pub struct RealtimeScope;

/// No-op — see [`RealtimeScope`].
#[cfg(not(all(rtsan, rtsan_standalone)))]
#[inline(always)]
pub const fn realtime_scope() -> RealtimeScope {
    RealtimeScope
}
