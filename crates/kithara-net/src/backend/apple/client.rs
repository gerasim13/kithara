use async_trait::async_trait;
use bytes::Bytes;
use http::{Method as HttpMethod, Request, Response};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::CancelToken;
use url::Url;

use super::{
    request::{AppleRequest, Method},
    session::AppleSession,
};
use crate::{
    ByteStream, HttpBody, HttpTransport, NetError, NetOptions,
    metrics::ConnectionMetrics,
    transport::external_body,
    types::{AcceptEncodingPolicy, Headers},
};

mod kithara {
    pub(crate) use kithara_test_macros::flash;
}

pub(crate) struct Transport {
    session: AppleSession,
    cancel: CancelToken,
}

impl Transport {
    pub(crate) fn new<S>(
        options: &NetOptions,
        pools: PoolRegion<S>,
        cancel: CancelToken,
        metrics: &ConnectionMetrics,
    ) -> Self
    where
        S: HasPool<u8> + Send + Sync + 'static,
    {
        Self {
            session: AppleSession::new(options, pools, metrics.clone()),
            cancel,
        }
    }

    #[kithara::flash(io)]
    async fn send(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError> {
        let (parts, body) = request.into_parts();
        let method = match parts.method {
            HttpMethod::GET => Method::Get,
            HttpMethod::HEAD => Method::Head,
            HttpMethod::POST => Method::Post,
            method => {
                return Err(NetError::Protocol(format!(
                    "unsupported Apple method {method}"
                )));
            }
        };
        let url = Url::parse(&parts.uri.to_string())
            .map_err(|error| NetError::Network(error.to_string()))?;
        let mut headers = Headers::default();
        for (name, value) in &parts.headers {
            headers.insert(
                name.as_str(),
                value
                    .to_str()
                    .map_err(|error| NetError::Network(error.to_string()))?,
            );
        }
        let accept_encoding = if headers
            .get("accept-encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("identity"))
        {
            AcceptEncodingPolicy::Identity
        } else {
            AcceptEncodingPolicy::Configured
        };
        let body = matches!(method, Method::Post).then_some(body);
        let request = AppleRequest::new(&url, method, None, Some(headers), body, accept_encoding)?;
        let response = self.session.stream(request, self.cancel.clone()).await?;
        let Some(status) = response.status else {
            response.cancel();
            return Err(NetError::Network(format!(
                "NSURLSession returned a non-HTTP response for {url}"
            )));
        };
        let mut head = Response::builder().status(status);
        for (name, value) in response.headers.iter() {
            head = head.header(name, value);
        }
        let stream = ByteStream::from(response);
        head.body(external_body(stream.into_inner()))
            .map_err(|error| NetError::Network(error.to_string()))
    }
}

#[async_trait]
impl HttpTransport for Transport {
    async fn open(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError> {
        self.send(request).await
    }
}
