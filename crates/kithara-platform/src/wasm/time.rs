use std::pin::pin;

use futures::future::{self as future_util, Either};
use gloo_timers::future::TimeoutFuture;
use js_sys::Date;
pub use web_time::Instant;

pub use crate::common::time::{Duration, SystemTime, TimeoutError, WallInstant};

pub async fn sleep(duration: Duration) {
    let ms = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    TimeoutFuture::new(ms.cast_unsigned()).await;
}

/// Coarse `Date.now()` timestamp in milliseconds, safe in every wasm scope.
///
/// Unlike [`Instant`] (which reads `performance.now()` and traps in an
/// `AudioWorkletGlobalScope`, where `performance` is undefined), `Date.now()`
/// is valid on the audio render thread. wasm-only: native code with the same
/// need should use [`Instant`] directly, which is cheap there. Coarse
/// (millisecond, wall-clock) — for second-scale deadlines, not precise
/// interval timing.
#[must_use]
pub fn coarse_now_ms() -> f64 {
    Date::now()
}

pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, TimeoutError>
where
    F: Future,
{
    let deadline = pin!(sleep(duration));
    let work = pin!(future);

    match future_util::select(work, deadline).await {
        Either::Left((output, _)) => Ok(output),
        Either::Right(((), _)) => Err(TimeoutError),
    }
}

#[cfg(test)]
mod tests {
    use std::{pin::pin, task::Poll};

    use futures::{channel::oneshot, poll};
    use kithara_test_utils::kithara;
    use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

    use super::{Duration, sleep, timeout};

    #[wasm_bindgen(inline_js = r#"
export function watchTimers() {
    const set = globalThis.setTimeout;
    const clear = globalThis.clearTimeout;
    const active = new Set();
    globalThis.setTimeout = (callback, delay, ...args) => {
        const id = set.call(globalThis, () => {
            active.delete(id);
            callback(...args);
        }, delay);
        active.add(id);
        return id;
    };
    globalThis.clearTimeout = id => {
        active.delete(id);
        clear.call(globalThis, id);
    };
    return { active, set, clear };
}
export function activeTimers(watch) {
    return watch.active.size;
}
export function restoreTimers(watch) {
    for (const id of watch.active) watch.clear.call(globalThis, id);
    globalThis.setTimeout = watch.set;
    globalThis.clearTimeout = watch.clear;
}
"#)]
    extern "C" {
        #[wasm_bindgen(js_name = watchTimers)]
        fn watch_timers() -> JsValue;
        #[wasm_bindgen(js_name = activeTimers)]
        fn active_timers(watch: &JsValue) -> u32;
        #[wasm_bindgen(js_name = restoreTimers)]
        fn restore_timers(watch: &JsValue);
    }

    struct Timers(JsValue);

    impl Drop for Timers {
        fn drop(&mut self) {
            restore_timers(&self.0);
        }
    }

    #[kithara::test(wasm, serial, flash(false))]
    async fn dropping_a_pending_sleep_cancels_its_browser_timer() {
        let timers = Timers(watch_timers());
        let (first, armed) = {
            let mut pending = pin!(sleep(Duration::from_secs(1)));
            (poll!(pending.as_mut()), active_timers(&timers.0))
        };
        let remaining = active_timers(&timers.0);
        drop(timers);
        assert!(first.is_pending());
        assert_eq!(armed, 1, "sleep must arm a timer");
        assert_eq!(remaining, 0, "dropping sleep must clear its timer");
    }

    #[kithara::test(wasm, serial, flash(false))]
    async fn completed_timeout_cancels_its_pending_browser_deadline() {
        let timers = Timers(watch_timers());
        let (ready, work) = oneshot::channel();
        let mut pending = pin!(timeout(Duration::from_secs(1), work));
        let first = poll!(pending.as_mut());
        let armed = active_timers(&timers.0);
        let sent = ready.send(7);
        let completed = poll!(pending.as_mut());
        let remaining = active_timers(&timers.0);
        drop(timers);
        assert!(first.is_pending());
        assert_eq!(armed, 1, "pending timeout must arm its deadline");
        assert_eq!(sent, Ok(()));
        assert!(matches!(completed, Poll::Ready(Ok(Ok(7)))));
        assert_eq!(remaining, 0, "completed work must clear its deadline");
    }
}
