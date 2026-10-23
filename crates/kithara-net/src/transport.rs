use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{Request, Response};

use crate::{HttpBody, NetError};

mod kithara {
    pub(crate) use kithara_test_macros::flash;
}

/// Opens response headers and transfers ownership of the streaming body.
/// Dropping the future or body must cancel its outstanding delivery work.
/// The resource can move between workers; wasm request futures remain local.
/// External implementations must protect pending I/O with
/// [`kithara_platform::flash::real_io`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait HttpTransport: Send + Sync {
    /// # Errors
    ///
    /// Returns the typed transport failure without retrying the request.
    async fn open(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError>;
}

pub(crate) fn external_body(body: HttpBody) -> HttpBody {
    Box::pin(stream::unfold(body, |mut body| async move {
        external_chunk(&mut body).await.map(|chunk| (chunk, body))
    }))
}

#[kithara::flash(io)]
async fn external_chunk(body: &mut HttpBody) -> Option<Result<Bytes, NetError>> {
    body.next().await
}
