use std::{
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use kithara_platform::maybe_send::{MaybeSend, MaybeSync};
use url::Url;

#[cfg(not(target_arch = "wasm32"))]
mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

use crate::{
    error::NetError,
    types::{Headers, RangeSpec},
};

/// HTTP response body after the transport's native decoding.
///
/// On native: requires `Send` (multi-threaded tokio runtime).
/// On wasm32: no `Send` bound (JsValue-based streams are `!Send`).
#[cfg(not(target_arch = "wasm32"))]
pub type HttpBody = Pin<Box<dyn Stream<Item = Result<Bytes, NetError>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type HttpBody = Pin<Box<dyn Stream<Item = Result<Bytes, NetError>>>>;

/// HTTP byte stream with response headers.
///
/// Wraps a streaming body together with the HTTP response headers so that
/// callers can inspect metadata (e.g. `Content-Length`) without an extra
/// HEAD request.
///
/// Implements [`Stream`] by delegating to the inner body, so it can be
/// passed directly to any consumer that expects a byte stream.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct ByteStream {
    /// Response headers from the HTTP request that produced this stream.
    pub headers: Headers,
    inner: HttpBody,
    #[field(get = is_partial)]
    partial: bool,
}

impl ByteStream {
    /// Create a new `ByteStream` from response headers and a raw body stream.
    #[must_use]
    pub fn new(headers: Headers, inner: HttpBody) -> Self {
        Self::with_partial(headers, inner, false)
    }

    /// Consume the wrapper, returning just the raw byte stream.
    #[must_use = "the wrapper is consumed; dropping the stream discards the response body"]
    pub fn into_inner(self) -> HttpBody {
        self.inner
    }

    /// Create a new `ByteStream` and record whether the response was partial.
    #[must_use]
    pub fn with_partial(headers: Headers, inner: HttpBody, partial: bool) -> Self {
        Self {
            headers,
            inner,
            partial,
        }
    }
}

impl Stream for ByteStream {
    type Item = Result<Bytes, NetError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

/// HTTP networking trait.
///
/// Single definition for both native and wasm32 targets.
/// On native: `MaybeSend` = `Send`, `MaybeSync` = `Sync`, futures are `Send`.
/// On wasm32: `MaybeSend`/`MaybeSync` are blanket-implemented (no-op), futures are `!Send`.
#[cfg_attr(not(target_arch = "wasm32"), kithara::mock(api = NetMock))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Net: MaybeSend + MaybeSync {
    /// Get all bytes from a URL
    async fn get_bytes(&self, url: Url, headers: Option<Headers>) -> Result<Bytes, NetError>;

    /// Get a range of bytes from a URL
    async fn get_range(
        &self,
        url: Url,
        range: RangeSpec,
        headers: Option<Headers>,
    ) -> Result<ByteStream, NetError>;

    /// Perform a HEAD request.
    ///
    /// This is intended for lightweight metadata probes (e.g. `Content-Length`,
    /// `Accept-Ranges`, `Content-Type`). Implementations should return response headers.
    async fn head(&self, url: Url, headers: Option<Headers>) -> Result<Headers, NetError>;

    /// POST `body` to a URL and return the full response body.
    async fn post_bytes(
        &self,
        url: Url,
        body: Bytes,
        headers: Option<Headers>,
    ) -> Result<Bytes, NetError>;

    /// Stream bytes from a URL
    async fn stream(&self, url: Url, headers: Option<Headers>) -> Result<ByteStream, NetError>;
}
