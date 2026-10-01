use std::{
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;

pub use crate::{
    backend::time::{Duration, SystemTime, TimeoutError},
    common::time::WallInstant,
    flash::Instant,
};

/// `sleep` under `flash` (native): the active mode is read on the thread that
/// polls this future. Real mode uses a `tokio` timer; active flash mode registers
/// a virtual deadline on the quiescence engine. Consumers call this same API in
/// both modes.
pub async fn sleep(duration: Duration) {
    if crate::flash::flash_enabled() {
        crate::flash::FlashSleep::new(duration).await;
    } else {
        crate::backend::time::sleep(duration).await;
    }
}

/// Await `future` with a deadline on the SAME clock as the awaited work — under
/// `flash` an engine-backed virtual deadline (see the off-feature [`timeout`]
/// for the full contract).
///
/// # Errors
///
/// Returns [`TimeoutError`] if the future does not complete within `duration`.
pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, TimeoutError>
where
    F: Future,
{
    if crate::flash::flash_enabled() {
        FlashTimeout {
            future,
            sleep: crate::flash::FlashSleep::new(duration),
        }
        .await
    } else {
        crate::backend::time::timeout(duration, future).await
    }
}

pin_project! {
    /// Races `future` against an engine-backed [`crate::flash::FlashSleep`] deadline
    /// (see the `flash` [`timeout`]). The deadline is ARMED before `future` is
    /// polled: the engine dates a deadline from the clock it reads at
    /// registration, and the guarded work registers waits of its own, so arming
    /// afterwards would date the deadline from a clock that work had already
    /// moved - a longer inner wait would then outlive the shorter timeout. Once
    /// armed, the future is polled first, so a ready result wins a tie with the
    /// deadline. `pub(crate)`: also constructed by the platform's clock tests.
    pub(crate) struct FlashTimeout<F> {
        #[pin]
        pub(crate) future: F,
        #[pin]
        pub(crate) sleep: crate::flash::FlashSleep,
    }
}

impl<F: Future> Future for FlashTimeout<F> {
    type Output = Result<F::Output, TimeoutError>;

    /// Polls the inner future before the sleep, so a ready result wins a tie against a
    /// simultaneously expired timeout.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        this.sleep.as_mut().arm(cx);
        if let Poll::Ready(out) = this.future.poll(cx) {
            return Poll::Ready(Ok(out));
        }
        match this.sleep.poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(TimeoutError)),
            Poll::Pending => Poll::Pending,
        }
    }
}
