use std::io::Read;

use kithara_platform::time::Duration;
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{CONTENT_RANGE, RANGE},
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::hls_hydrate::{Deadline, HydrateError, RedactedUrl};

#[derive(Debug, Error)]
pub(crate) enum RemoteFileError {
    #[error("repository variable {0} is missing")]
    Missing(&'static str),
    #[error(transparent)]
    Fetch(#[from] HydrateError),
    #[error("invalid remote fixture URL")]
    Url(#[from] url::ParseError),
    #[error("{url}: expected {expected} bytes, received {received}")]
    Length {
        url: RedactedUrl,
        expected: u64,
        received: u64,
    },
    #[error("{url}: answered {status} instead of resuming at byte {offset}")]
    Resume {
        url: RedactedUrl,
        offset: u64,
        status: StatusCode,
    },
    #[error("{url}: SHA-256 mismatch, expected {expected}, received {received}")]
    Digest {
        url: RedactedUrl,
        expected: String,
        received: String,
    },
}

/// Downloads one public file and verifies its size and SHA-256 digest.
///
/// A request or transfer silent for `stall` is asked again from the byte it
/// reached: a CDN edge can hold a request or a response open without sending
/// anything, and one such stall must not spend the whole `timeout`.
pub(crate) fn fetch_verified(
    url: &Url,
    sha256_hex: &str,
    length: u64,
    timeout: Duration,
    stall: Duration,
) -> Result<Vec<u8>, RemoteFileError> {
    // A blocking client's own timeout bounds each wait, the response and every
    // read, so it measures silence rather than the whole transfer.
    let client =
        Client::builder()
            .timeout(stall)
            .build()
            .map_err(|source| HydrateError::Request {
                url: RedactedUrl::new(url),
                source: source.without_url(),
            })?;
    let deadline = Deadline::new(timeout);
    let mut bytes = Vec::new();
    loop {
        let Some(mut response) = request(&client, url, bytes.len() as u64, deadline)? else {
            continue;
        };
        // An interrupted body keeps what arrived: the next request resumes
        // after it.
        if response.read_to_end(&mut bytes).is_ok() {
            break;
        }
    }
    let received = bytes.len() as u64;
    if received != length {
        return Err(RemoteFileError::Length {
            received,
            url: RedactedUrl::new(url),
            expected: length,
        });
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != sha256_hex {
        return Err(RemoteFileError::Digest {
            url: RedactedUrl::new(url),
            expected: sha256_hex.to_owned(),
            received: digest,
        });
    }
    Ok(bytes)
}

/// Requests `url` from byte `offset` on, refusing an answer that restarts or
/// skips the body instead of continuing it; `None` when the request went
/// unanswered for the client's stall timeout.
fn request(
    client: &Client,
    url: &Url,
    offset: u64,
    deadline: Deadline,
) -> Result<Option<Response>, RemoteFileError> {
    deadline.remaining(url)?;
    let mut request = client.get(url.clone());
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(source) if source.is_timeout() => return Ok(None),
        Err(source) => {
            return Err(HydrateError::Request {
                url: RedactedUrl::new(url),
                source: source.without_url(),
            }
            .into());
        }
    };
    let status = response.status();
    if !status.is_success() {
        return Err(HydrateError::Status {
            status,
            url: RedactedUrl::new(url),
        }
        .into());
    }
    let resumes_here = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|range| range.to_str().ok())
        .is_some_and(|range| range.starts_with(&format!("bytes {offset}-")));
    if offset > 0 && (status != StatusCode::PARTIAL_CONTENT || !resumes_here) {
        return Err(RemoteFileError::Resume {
            url: RedactedUrl::new(url),
            offset,
            status,
        });
    }
    Ok(Some(response))
}
