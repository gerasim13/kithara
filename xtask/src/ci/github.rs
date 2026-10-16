use anyhow::{Context, Result};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};

use crate::consts;

/// A REST client that authenticates every request with `token`.
pub(crate) fn client(token: &str) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("kithara-ci"));
    headers.insert(consts::API_VERSION, HeaderValue::from_static("2022-11-28"));
    let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
        .context("the GitHub token is not a usable header value")?;
    authorization.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);
    Client::builder()
        .default_headers(headers)
        .build()
        .context("building the GitHub client")
}
