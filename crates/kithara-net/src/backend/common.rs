use std::num::NonZeroU16;

use bytes::Bytes;
use url::Url;

use crate::{
    error::{NetError, truncate_error_body},
    types::Headers,
};

pub(crate) fn status_error(url: Url, status: u16, body: &Bytes) -> NetError {
    let body = if body.is_empty() {
        None
    } else {
        Some(truncate_error_body(
            String::from_utf8_lossy(body).into_owned(),
            "...",
        ))
    };
    match NonZeroU16::new(status) {
        Some(status) => NetError::Status {
            status,
            body,
            url: Some(url),
        },
        None => NetError::Network(format!("unexpected zero HTTP status for {url}")),
    }
}

/// The `content-encoding` value when it names any coding besides `identity`:
/// a success body that carries one reached the caller still encoded.
pub(crate) fn non_identity_content_encoding(pairs: &[(String, String)]) -> Option<&str> {
    pairs.iter().find_map(|(key, value)| {
        (key.eq_ignore_ascii_case("content-encoding")
            && value
                .split(',')
                .map(str::trim)
                .any(|coding| !coding.eq_ignore_ascii_case("identity")))
        .then_some(value.as_str())
    })
}

/// A 206 to a probe states the representation total in content-range alone.
pub(crate) fn normalize_head_headers(mut headers: Headers) -> Headers {
    if headers.get("content-length").is_none()
        && let Some(total) = content_length_from_range(&headers)
    {
        headers.insert("content-length", total);
    }
    headers
}

fn content_length_from_range(headers: &Headers) -> Option<String> {
    headers
        .get("content-range")
        .and_then(|header| header.split('/').nth(1))
        .filter(|total| *total != "*")
        .map(str::to_owned)
}
