use std::num::NonZeroU16;

use url::Url;

use crate::{error::NetError, types::Headers};

pub(crate) fn status_error(url: Url, status: u16, body: String) -> NetError {
    match NonZeroU16::new(status) {
        Some(status) => NetError::Status {
            status,
            url: Some(url),
            body: Some(body),
        },
        None => NetError::Network(format!("unexpected zero HTTP status for {url}")),
    }
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
