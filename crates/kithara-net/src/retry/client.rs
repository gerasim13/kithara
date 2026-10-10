use async_trait::async_trait;
use bytes::Bytes;
use kithara_platform::sync::Arc;
use url::Url;

use super::net::RetryNet;
use crate::{
    ByteStream,
    error::{NetError, NetResult},
    metrics::ConnectionMetrics,
    traits::Net,
    types::{Headers, RangeSpec},
};

#[derive_where::derive_where(Clone)]
pub struct RetryClient<Raw> {
    pub(crate) net: Arc<RetryNet<Raw>>,
    pub(crate) connection_metrics: ConnectionMetrics,
}

impl<Raw: Net> RetryClient<Raw> {
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connection_metrics.connection_count()
    }

    delegate::delegate! {
        to self.net {
            /// # Errors
            ///
            /// Returns [`NetError`] on HTTP failure, timeout, or network error.
            pub async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> NetResult<Bytes>;
            /// # Errors
            ///
            /// Returns [`NetError`] on HTTP failure or network error.
            pub async fn get_range(
                &self,
                url: Url,
                range: RangeSpec,
                headers: Option<Headers>,
            ) -> NetResult<ByteStream>;
            /// # Errors
            ///
            /// Returns [`NetError`] on HTTP failure or network error.
            pub async fn head(&self, url: Url, headers: Option<Headers>) -> NetResult<Headers>;
            /// # Errors
            ///
            /// Returns [`NetError`] on HTTP failure, timeout, or network error.
            pub async fn post_bytes(
                &self,
                url: Url,
                body: Bytes,
                headers: Option<Headers>,
            ) -> NetResult<Bytes>;
            /// # Errors
            ///
            /// Returns [`NetError`] on HTTP failure or network error.
            pub async fn stream(&self, url: Url, headers: Option<Headers>) -> NetResult<ByteStream>;
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl<Raw: Net> Net for RetryClient<Raw> {
    async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> Result<Bytes, NetError> {
        self.net.get_bytes(url, headers).await
    }

    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> Result<ByteStream, NetError> {
        self.net.get_range(url, range, headers).await
    }

    async fn head(&self, url: Url, headers: Option<Headers>) -> Result<Headers, NetError> {
        self.net.head(url, headers).await
    }

    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> Result<Bytes, NetError> {
        self.net.post_bytes(url, body, headers).await
    }

    async fn stream(&self, url: Url, headers: Option<Headers>) -> Result<ByteStream, NetError> {
        self.net.stream(url, headers).await
    }
}
