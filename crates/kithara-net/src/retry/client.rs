use async_trait::async_trait;
use bytes::Bytes;
use kithara_platform::{
    maybe_send::{MaybeSend, MaybeSync},
    sync::Arc,
};
use url::Url;

use super::net::RetryNet;
use crate::{
    ByteStream,
    error::{NetError, NetResult},
    metrics::ConnectionMetrics,
    observe::Observer,
    traits::Net,
    types::{Headers, RangeSpec},
};

/// One attempt of each request on a raw backend, reported to the request's observer.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait RawNet: MaybeSend + MaybeSync {
    async fn get_bytes(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Bytes, NetError>;

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError>;

    async fn head(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Headers, NetError>;

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<Bytes, NetError>;

    async fn stream(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> Result<ByteStream, NetError>;
}

#[derive_where::derive_where(Clone)]
pub struct RetryClient<Raw> {
    pub(crate) net: Arc<RetryNet<Raw>>,
    pub(crate) connection_metrics: ConnectionMetrics,
    pub(crate) observer: Option<Observer>,
}

impl<Raw: RawNet> RetryClient<Raw> {
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connection_metrics.connection_count()
    }

    /// Returns a handle sharing this client's transport, options and retry
    /// policy that reports its requests to `observer`.
    #[must_use]
    pub fn with_observer(&self, observer: Option<Observer>) -> Self {
        Self {
            observer,
            ..self.clone()
        }
    }

    /// # Errors
    ///
    /// Returns [`NetError`] on HTTP failure, timeout, or network error.
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
    /// Returns [`NetError`] on HTTP failure or network error.
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
    /// Returns [`NetError`] on HTTP failure or network error.
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
    /// Returns [`NetError`] on HTTP failure, timeout, or network error.
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
    /// Returns [`NetError`] on HTTP failure or network error.
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

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl<Raw: RawNet> Net for RetryClient<Raw> {
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
