mod error;
#[cfg(not(target_arch = "wasm32"))]
mod metrics;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "client-wreq")))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
mod shared;
#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(all(not(target_arch = "wasm32"), feature = "client-wreq"))]
mod wreq;

#[cfg(all(not(target_arch = "wasm32"), not(feature = "client-wreq")))]
pub(crate) use self::native::{
    BackendError, Client, ClientBuilder, RequestBuilder, Response, StatusCode, build_client,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use self::shared::{apply_compression, head_request, post_request};
#[cfg(target_arch = "wasm32")]
pub(crate) use self::wasm::{
    BackendError, Client, RequestBuilder, Response, StatusCode, build_client, head_request,
    post_request,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "client-wreq"))]
pub(crate) use self::wreq::{
    BackendError, Client, ClientBuilder, RequestBuilder, Response, StatusCode, build_client,
};
pub use crate::client::HttpClient;
