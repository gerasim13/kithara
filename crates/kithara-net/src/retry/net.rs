use std::future::Future;

use kithara_platform::{CancelToken, time::sleep, tokio};

use crate::{
    error::{NetError, Retryability},
    observe::Observer,
    types::RetryPolicy,
};

/// Retry loop shared by the client handles.
pub(crate) struct RetryNet<N> {
    cancel: CancelToken,
    inner: N,
    retry_policy: RetryPolicy,
}

impl<N> RetryNet<N> {
    delegate::delegate! {
        to self {
            #[field(&inner)]
            pub(crate) const fn inner(&self) -> &N;
        }
    }

    pub(crate) const fn new(inner: N, retry_policy: RetryPolicy, cancel: CancelToken) -> Self {
        Self {
            cancel,
            inner,
            retry_policy,
        }
    }

    /// Promotes a transient error to a terminal `RetryExhausted` once a non-zero retry budget has
    /// been spent, so downstream (HLS settle, readers) treats it as a give-up rather than a
    /// transient signal.
    pub(crate) async fn retry_loop<F, Fut, T>(
        &self,
        observer: Option<&Observer>,
        mut op: F,
    ) -> Result<T, NetError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, NetError>>,
    {
        let max = self.retry_policy.max_retries;
        let mut attempt = 0;
        loop {
            let error = match op().await {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            if !self.retry_policy.should_retry(&error, attempt) {
                return Err(
                    if max > 0 && error.retryability() == Retryability::Transient {
                        if let Some(observer) = observer {
                            observer.0.retry_exhausted(max, 0, &error);
                        }
                        NetError::RetryExhausted {
                            max_retries: max,
                            source: Box::new(error),
                        }
                    } else {
                        error
                    },
                );
            }
            let delay = self.retry_policy.delay_for_attempt(attempt);
            if let Some(observer) = observer {
                observer.0.retrying(attempt + 1, max, &error, delay);
            }
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(NetError::Cancelled),
                () = sleep(delay) => {}
            }
            attempt += 1;
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    mod kithara {
        pub(crate) use kithara_test_macros::test;
    }

    use std::num::NonZeroU16;

    use super::*;

    fn status_404() -> NetError {
        NetError::Status {
            status: NonZeroU16::new(404).expect("BUG: 404 is non-zero"),
            url: None,
            body: None,
        }
    }

    #[kithara::test]
    #[case(0, true, "first attempt should retry")]
    #[case(1, true, "second attempt should retry")]
    #[case(2, true, "third attempt should retry")]
    #[case(3, false, "fourth attempt should not retry (max=3)")]
    #[case(4, false, "fifth attempt should not retry")]
    fn test_default_retry_policy_should_retry_max_retries(
        #[case] attempt: u32,
        #[case] expected: bool,
        #[case] _desc: &str,
    ) {
        let policy = RetryPolicy::default();
        let error = NetError::Timeout;
        assert_eq!(policy.should_retry(&error, attempt), expected);
    }

    #[kithara::test]
    fn test_default_retry_policy_should_not_retry_non_retryable() {
        let policy = RetryPolicy::default();
        let error = status_404();
        assert!(!policy.should_retry(&error, 0));
    }
}
