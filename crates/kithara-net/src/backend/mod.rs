mod guards;

#[cfg(not(reqwest_backend))]
mod common;
#[cfg(not(reqwest_backend))]
mod pooled;

#[cfg(feature = "client-host")]
pub(crate) mod host;
#[cfg(feature = "client-host")]
pub use self::host::HttpClient;

#[cfg(apple_backend)]
mod apple;
#[cfg(apple_backend)]
pub use self::apple::HttpClient;

#[cfg(reqwest_backend)]
mod reqwest;
#[cfg(reqwest_backend)]
pub use self::reqwest::HttpClient;
#[cfg(reqwest_backend)]
pub(crate) use self::reqwest::{
    Client, RequestBuilder, Response, StatusCode, build_client, head_request, post_request,
};
