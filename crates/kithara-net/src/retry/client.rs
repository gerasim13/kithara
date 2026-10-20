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
    error::NetResult,
    metrics::ConnectionMetrics,
    observe::Observer,
    traits::Net,
    types::{Headers, RangeSpec},
};

/// Raw HTTP operations using the observer of the requesting handle.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait RawBackend: MaybeSend + MaybeSync {
    async fn get_bytes(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> NetResult<Bytes>;
    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> NetResult<ByteStream>;
    async fn head(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> NetResult<Headers>;
    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> NetResult<Bytes>;
    async fn stream(
        &self,
        url: Url,
        headers: Option<Headers>,
        observer: Option<&Observer>,
    ) -> NetResult<ByteStream>;
}

#[derive_where::derive_where(Clone)]
pub struct RetryClient<Raw> {
    pub(crate) net: Arc<RetryNet<Raw>>,
    pub(crate) connection_metrics: ConnectionMetrics,
    pub(crate) observer: Option<Observer>,
}

impl<Raw: RawBackend> RetryClient<Raw> {
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connection_metrics.connection_count()
    }

    /// # Errors
    /// Returns [`crate::NetError`] on HTTP failure, timeout, or network error.
    pub async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> NetResult<Bytes> {
        self.net
            .retry_loop(self.observer.as_ref(), || {
                self.net
                    .inner()
                    .get_bytes(url.clone(), headers.clone(), self.observer.as_ref())
            })
            .await
    }

    /// # Errors
    /// Returns [`crate::NetError`] on HTTP failure or network error.
    pub async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> NetResult<ByteStream> {
        self.net
            .retry_loop(self.observer.as_ref(), || {
                self.net.inner().get_range(
                    url.clone(),
                    range.clone(),
                    headers.clone(),
                    self.observer.as_ref(),
                )
            })
            .await
    }

    /// # Errors
    /// Returns [`crate::NetError`] on HTTP failure or network error.
    pub async fn head(&self, url: Url, headers: Option<Headers>) -> NetResult<Headers> {
        self.net
            .retry_loop(self.observer.as_ref(), || {
                self.net
                    .inner()
                    .head(url.clone(), headers.clone(), self.observer.as_ref())
            })
            .await
    }

    /// # Errors
    /// Returns [`crate::NetError`] on HTTP failure, timeout, or network error.
    pub async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> NetResult<Bytes> {
        self.net
            .retry_loop(self.observer.as_ref(), || {
                self.net.inner().post_bytes(
                    url.clone(),
                    body.clone(),
                    headers.clone(),
                    self.observer.as_ref(),
                )
            })
            .await
    }

    /// # Errors
    /// Returns [`crate::NetError`] on HTTP failure or network error.
    pub async fn stream(&self, url: Url, headers: Option<Headers>) -> NetResult<ByteStream> {
        self.net
            .retry_loop(self.observer.as_ref(), || {
                self.net
                    .inner()
                    .stream(url.clone(), headers.clone(), self.observer.as_ref())
            })
            .await
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl<Raw: RawBackend> Net for RetryClient<Raw> {
    async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> NetResult<Bytes> {
        Self::get_bytes(self, url, headers).await
    }

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> NetResult<ByteStream> {
        Self::get_range(self, url, range, headers).await
    }

    async fn head(&self, url: Url, headers: Option<Headers>) -> NetResult<Headers> {
        Self::head(self, url, headers).await
    }

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> NetResult<Bytes> {
        Self::post_bytes(self, url, body, headers).await
    }

    async fn stream(&self, url: Url, headers: Option<Headers>) -> NetResult<ByteStream> {
        Self::stream(self, url, headers).await
    }
}
