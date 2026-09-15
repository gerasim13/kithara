//! Memory contract of the tracing USDT backend: probes fired without pause
//! keep the heap bounded whether nothing observes them or a scope records
//! them far past its history cap.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use kithara_test_utils::{
    test::{
        setup_tracing,
        usdt::{MAX_EVENTS, ProbeEvent, scope},
    },
    tracing::{Level, event},
};

/// Heap bytes live right now, and the highest value since the last reset.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counters only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded verbatim to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarded verbatim to the system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const THREADS: usize = 4;
/// Firings per thread before measuring: fills the flight recorder's rings and
/// every per-thread tracing buffer, which are bounded but not preallocated.
const WARMUP: u64 = 20_000;
/// Firings per thread measured: far past every bound the backend keeps.
const FIRINGS: u64 = 1_000_000;
/// Growth allowed once warm while nothing keeps history: allocator and
/// ring-entry jitter, never a per-firing cost.
const STEADY_BUDGET: usize = 256 * 1024;

fn fire(probe: &'static str, value: u64) {
    event!(
        target: "kithara_test_probe",
        Level::TRACE,
        probe = probe,
        value = value,
    );
}

/// Warms up on `warm` then fires `probe` from [`THREADS`] threads, returning
/// the peak heap growth over the warm baseline and the growth left after the
/// threads finish.
fn hammer(warm_probe: &'static str, probe: &'static str) -> (usize, usize) {
    let warm = Barrier::new(THREADS + 1);
    let go = Barrier::new(THREADS + 1);
    let mut baseline = 0;
    thread::scope(|threads| {
        for _ in 0..THREADS {
            threads.spawn(|| {
                for value in 0..WARMUP {
                    fire(warm_probe, value);
                }
                warm.wait();
                go.wait();
                for value in 0..FIRINGS {
                    fire(probe, value);
                }
            });
        }
        warm.wait();
        baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        go.wait();
    });
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let left = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    (peak, left)
}

/// Loads the symbol cache std keeps for the rest of the process the first
/// time a panic prints a backtrace, so the overflow panic below cannot read
/// as heap a dropped scope left behind.
fn prime_panic_backtrace() {
    let primed = catch_unwind(|| panic!("priming the panic backtrace cache"));
    assert!(primed.is_err());
}

#[test]
fn continuous_probes_keep_the_heap_bounded() {
    setup_tracing();
    prime_panic_backtrace();

    let (peak, left) = hammer("unobserved", "unobserved");
    eprintln!("unobserved: peak +{peak} B, left +{left} B");
    assert!(
        peak <= STEADY_BUDGET,
        "unobserved probes grew the heap by {peak} B"
    );

    let before = LIVE.load(Ordering::Relaxed);
    let history = scope();
    let (peak, _) = hammer("unobserved", "history");
    let history_bytes = MAX_EVENTS * size_of::<ProbeEvent>();
    assert!(
        peak >= history_bytes,
        "a scope past MAX_EVENTS must have held the full history, peak {peak} B"
    );
    let history_budget = history_bytes + history_bytes / 2 + STEADY_BUDGET;
    eprintln!(
        "history: peak +{peak} B for {MAX_EVENTS} events of {} B (budget {history_budget} B)",
        size_of::<ProbeEvent>()
    );
    assert!(
        peak <= history_budget,
        "a scope grew the heap by {peak} B, over {history_budget} B"
    );
    let overflow = catch_unwind(AssertUnwindSafe(|| history.events().len()));
    assert!(
        overflow.is_err(),
        "a history past MAX_EVENTS must fail its reader"
    );
    assert!(
        history.last("history").is_some(),
        "an overflowed history must keep the latest firing"
    );
    drop(history);

    let left = LIVE.load(Ordering::Relaxed).saturating_sub(before);
    eprintln!("after the scopes: left +{left} B");
    assert!(
        left <= STEADY_BUDGET,
        "a dropped scope left {left} B on the heap"
    );
}
