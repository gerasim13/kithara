use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use http::{Request, Response};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::CancelToken;

use super::{Client, build_client};
use crate::{
    HttpBody, HttpTransport, NetError, NetOptions, metrics::ConnectionMetrics,
    transport::external_body,
};

mod kithara {
    pub(crate) use kithara_test_macros::flash;
}

pub(crate) struct Transport {
    client: Client,
    #[cfg(not(target_arch = "wasm32"))]
    accept_encoding: String,
}

impl Transport {
    pub(crate) fn new<S>(
        options: &NetOptions,
        _pools: PoolRegion<S>,
        _cancel: CancelToken,
        metrics: &ConnectionMetrics,
    ) -> Self
    where
        S: HasPool<u8> + Send + Sync + 'static,
    {
        Self {
            client: build_client(options, metrics)
                .expect("BUG: HTTP client builder with our defaults cannot fail"),
            #[cfg(not(target_arch = "wasm32"))]
            accept_encoding: crate::types::accept_encoding_value(options.compression),
        }
    }

    #[kithara::flash(io)]
    async fn send(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError> {
        let (parts, body) = request.into_parts();
        #[cfg(target_arch = "wasm32")]
        let mut parts = parts;
        #[cfg(target_arch = "wasm32")]
        if parts.method == http::Method::HEAD {
            parts.method = http::Method::GET;
            parts
                .headers
                .insert("range", http::HeaderValue::from_static("bytes=0-0"));
        }
        let has_body = parts.method == http::Method::POST || !body.is_empty();
        let mut request = self.client.request(parts.method, parts.uri.to_string());
        for (name, value) in &parts.headers {
            #[cfg(target_arch = "wasm32")]
            if name == "accept-encoding" {
                continue;
            }
            request = request.header(name, value);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !parts.headers.contains_key("accept-encoding") {
            request = request.header("Accept-Encoding", &self.accept_encoding);
        }
        if has_body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(NetError::from)?;
        let mut head = Response::builder().status(response.status());
        for (name, value) in response.headers() {
            head = head.header(name, value);
        }
        let body = external_body(Box::pin(response.bytes_stream().map_err(NetError::from)));
        head.body(body)
            .map_err(|error| NetError::Network(error.to_string()))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl HttpTransport for Transport {
    async fn open(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError> {
        self.send(request).await
    }
}
