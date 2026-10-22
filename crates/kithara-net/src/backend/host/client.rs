use async_trait::async_trait;
use bytes::Bytes;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::{
    CancelToken,
    sync::Arc,
    time::{Instant, timeout},
};
use url::Url;

use super::{
    exchange::{Exchange, Opened, Target},
    protocol::{check_status, consts::HTTP_PARTIAL_CONTENT},
    transport::HostMethod,
};
use crate::{
    ByteStream,
    backend::{common::normalize_head_headers, pooled::ByteBuffers},
    error::{NetError, NetResult},
    observe::Observer,
    range_response::validate_range_response,
    resumable::{Refetch, Resumed, resumable_body},
    retry::RetryNet,
    traits::Net,
    types::{AcceptEncodingPolicy, Headers, NetOptions, RangeSpec, RetryPolicy},
};

mod kithara {
    pub(crate) use kithara_test_macros::flash;
}

struct HostDataResponse {
    body: Bytes,
    headers: Headers,
    status: u16,
}

#[derive(Clone)]
struct RawHostNet {
    exchange: Exchange,
    options: NetOptions,
}

impl RawHostNet {
    async fn body_stream(
        &self,
        url: Url,
        range: Option<RangeSpec>,
        headers: Option<Headers>,
        accept_partial: bool,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError> {
        let base_start = range.as_ref().map_or(0, |range| range.start);
        let end = range.as_ref().and_then(|range| range.end);
        let first = self
            .raw_body(&url, range, headers.as_ref(), accept_partial, observer)
            .await?;
        Ok(self.wrap_resumable(first, url, base_start, end, headers, observer))
    }

    #[kithara::flash(io)]
    async fn data(
        &self,
        method: HostMethod,
        url: Url,
        body: Option<Bytes>,
        headers: Option<Headers>,
        accept_partial: bool,
        observer: Option<&Observer>,
    ) -> Result<HostDataResponse, NetError> {
        let started = Instant::now();
        let response = timeout(self.options.inactivity_timeout, async {
            let Opened {
                call,
                headers,
                status,
            } = self
                .exchange
                .open(Target {
                    method,
                    accept_encoding: AcceptEncodingPolicy::from(method),
                    url: &url,
                    range: None,
                    headers: headers.as_ref(),
                    body,
                })
                .await?;
            let body = self.exchange.drain(call).await?;
            Ok::<_, NetError>(HostDataResponse {
                body,
                headers,
                status,
            })
        })
        .await
        .map_err(|_| NetError::Timeout)??;

        Self::observe_first_byte(observer, started, response.status);
        check_status(&url, response.status, &response.body, accept_partial)?;
        Ok(response)
    }

    fn observe_first_byte(observer: Option<&Observer>, started: Instant, status: u16) {
        if let Some(observer) = observer {
            observer
                .0
                .first_byte(started.elapsed(), status, status == HTTP_PARTIAL_CONTENT);
        }
    }

    #[kithara::flash(io)]
    async fn raw_body(
        &self,
        url: &Url,
        range: Option<RangeSpec>,
        headers: Option<&Headers>,
        accept_partial: bool,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError> {
        let started = Instant::now();
        let opened = timeout(
            self.options.inactivity_timeout,
            self.exchange.open(Target {
                url,
                headers,
                method: HostMethod::Get,
                accept_encoding: AcceptEncodingPolicy::Identity,
                range: range.as_ref(),
                body: None,
            }),
        )
        .await
        .map_err(|_| NetError::Timeout)??;

        Self::observe_first_byte(observer, started, opened.status);
        let status = check_status(url, opened.status, &Bytes::new(), accept_partial)?;
        validate_range_response(status, range.as_ref(), &opened.headers, url)?;

        let partial = status == HTTP_PARTIAL_CONTENT;
        let Opened { call, headers, .. } = opened;
        let body = self.exchange.body(call);
        Ok(ByteStream::with_partial(headers, Box::pin(body), partial))
    }

    fn wrap_resumable(
        &self,
        first: ByteStream,
        url: Url,
        base_start: u64,
        end: Option<u64>,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> ByteStream {
        let out_headers = first.headers.clone();
        let partial = first.is_partial();
        let me = self.clone();
        let resource = url.clone();
        let observer = observer.cloned();
        let refetch_observer = observer.clone();
        let (resume_base, resume_end) = if partial {
            (base_start, end)
        } else {
            (0, None)
        };
        let refetch: Refetch = Box::new(move |consumed| {
            let me = me.clone();
            let url = url.clone();
            let headers = headers.clone();
            let observer = refetch_observer.clone();
            let abs = resume_base.saturating_add(consumed);
            let resume = RangeSpec::new(abs, resume_end);
            Box::pin(async move {
                let stream = me
                    .raw_body(
                        &url,
                        Some(resume),
                        headers.as_ref(),
                        true,
                        observer.as_ref(),
                    )
                    .await?;
                let skip = if stream.is_partial() { 0 } else { abs };
                Ok(Resumed { stream, skip })
            })
        });
        let body = resumable_body(
            first,
            refetch,
            resource,
            self.options.inactivity_timeout,
            self.options.retry_policy,
            self.exchange.cancel.clone(),
            observer,
        );
        ByteStream::with_partial(out_headers, body, partial)
    }

    async fn get_bytes(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Bytes, NetError> {
        self.data(HostMethod::Get, url, None, headers, false, observer)
            .await
            .map(|response| response.body)
    }

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError> {
        self.body_stream(url, Some(range), headers, true, observer)
            .await
    }

    async fn head(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Headers, NetError> {
        self.data(HostMethod::Head, url, None, headers, true, observer)
            .await
            .map(|response| normalize_head_headers(response.headers))
    }

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Bytes, NetError> {
        self.data(HostMethod::Post, url, Some(body), headers, false, observer)
            .await
            .map(|response| response.body)
    }

    async fn stream(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError> {
        self.body_stream(url, None, headers, false, observer).await
    }
}

/// Every request, through the transport installed in the process.
#[derive(Clone)]
pub struct HostNet {
    net: Arc<RetryNet<RawHostNet>>,
    observer: Option<Observer>,
}

impl HostNet {
    #[must_use]
    pub fn new<S>(options: NetOptions, pools: PoolRegion<S>, cancel: CancelToken) -> Self
    where
        S: HasPool<u8> + Send + Sync + 'static,
    {
        Self::from(RawHostNet {
            options,
            exchange: Exchange {
                cancel,
                buffers: ByteBuffers::new(pools),
            },
        })
    }

    #[must_use]
    pub fn with_observer(&self, observer: Option<Observer>) -> Self {
        Self {
            observer,
            ..self.clone()
        }
    }

    /// Returns a handle with a different retry policy, sharing the host transport.
    #[must_use]
    pub fn with_retry_policy(&self, retry_policy: RetryPolicy) -> Self {
        let mut options = self.options().clone();
        options.retry_policy = retry_policy;
        self.with_options(options)
    }

    fn with_options(&self, options: NetOptions) -> Self {
        Self {
            observer: self.observer.clone(),
            ..Self::from(RawHostNet {
                options,
                exchange: self.net.inner().exchange.clone(),
            })
        }
    }

    delegate::delegate! {
        to self.net.inner() {
            #[must_use]
            #[field(&options)]
            pub fn options(&self) -> &NetOptions;
        }
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, timeout, cancellation, or network error.
    pub async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> NetResult<Bytes> {
        let observer = self.observer.as_ref();
        self.net
            .retry_loop(observer, || {
                self.net
                    .inner()
                    .get_bytes(url.clone(), headers.clone(), observer)
            })
            .await
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, cancellation, or network error.
    pub async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> NetResult<ByteStream> {
        let observer = self.observer.as_ref();
        self.net
            .retry_loop(observer, || {
                self.net
                    .inner()
                    .get_range(url.clone(), range.clone(), headers.clone(), observer)
            })
            .await
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, cancellation, or network error.
    pub async fn head(&self, url: Url, headers: Option<Headers>) -> NetResult<Headers> {
        let observer = self.observer.as_ref();
        self.net
            .retry_loop(observer, || {
                self.net
                    .inner()
                    .head(url.clone(), headers.clone(), observer)
            })
            .await
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, timeout, cancellation, or network error.
    pub async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> NetResult<Bytes> {
        let observer = self.observer.as_ref();
        self.net
            .retry_loop(observer, || {
                self.net
                    .inner()
                    .post_bytes(url.clone(), body.clone(), headers.clone(), observer)
            })
            .await
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, cancellation, or network error.
    pub async fn stream(&self, url: Url, headers: Option<Headers>) -> NetResult<ByteStream> {
        let observer = self.observer.as_ref();
        self.net
            .retry_loop(observer, || {
                self.net
                    .inner()
                    .stream(url.clone(), headers.clone(), observer)
            })
            .await
    }
}

impl From<RawHostNet> for HostNet {
    fn from(raw: RawHostNet) -> Self {
        let retry_policy = raw.options.retry_policy;
        let cancel = raw.exchange.cancel.clone();
        let net = Arc::new(RetryNet::new(raw, retry_policy, cancel));
        Self {
            net,
            observer: None,
        }
    }
}

impl std::fmt::Debug for HostNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostNet")
            .field("options", self.options())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Net for HostNet {
    async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> Result<Bytes, NetError> {
        Self::get_bytes(self, url, headers).await
    }

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> Result<ByteStream, NetError> {
        Self::get_range(self, url, range, headers).await
    }

    async fn head(&self, url: Url, headers: Option<Headers>) -> Result<Headers, NetError> {
        Self::head(self, url, headers).await
    }

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> Result<Bytes, NetError> {
        Self::post_bytes(self, url, body, headers).await
    }

    async fn stream(&self, url: Url, headers: Option<Headers>) -> Result<ByteStream, NetError> {
        Self::stream(self, url, headers).await
    }
}
