use std::{fs::File, io::Read, path::PathBuf};

use kithara_abr::Abr;
use kithara_download::{Downloader, FetchCmd, Peer};
use kithara_net::{Headers, NetError, NetResult};
use kithara_platform::{CancelScope, CancelToken, sync::Arc, tokio::task};
use thiserror::Error;
use url::Url;

use super::ArtifactDocument;
use crate::resource::ResourceSrc;

/// Largest document any artifact may arrive as. A prepared grid or waveform is
/// a summary of a track, not a second copy of it: anything past this is a
/// wrong URL, not a big artifact, and it is refused before it is buffered.
pub const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;

/// Why a prepared artifact never reached the resource it was configured for.
///
/// Every variant is terminal for that artifact: a track opened with an
/// explicit source asked for that artifact, so a failure to read it is
/// reported, never quietly turned into local analysis.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArtifactLoadError {
    /// The artifact was cancelled with the load it belonged to.
    #[error("the {kind} artifact load was cancelled")]
    Cancelled { kind: &'static str },
    /// The bytes arrived but are not a document of this kind.
    #[error("the {kind} artifact is not a document this build reads: {reason}")]
    Decode { kind: &'static str, reason: String },
    /// The bytes never arrived.
    #[error("the {kind} artifact at {src} could not be read: {reason}")]
    Fetch {
        kind: &'static str,
        src: String,
        reason: String,
    },
    /// A remote artifact needs the shared downloader the resource is wired
    /// with; without one there is no I/O path to read it over.
    #[error("no downloader is configured to read the {kind} artifact at {url}")]
    NoDownloader { kind: &'static str, url: Url },
    /// The document is past [`MAX_ARTIFACT_BYTES`].
    #[error("the {kind} artifact is {bytes} bytes, past the {limit} a document may hold")]
    TooLarge {
        kind: &'static str,
        bytes: u64,
        limit: usize,
    },
    /// The scheme is not one an artifact document is read over.
    #[error("{scheme} is not a scheme a {kind} artifact is read from")]
    Scheme { kind: &'static str, scheme: String },
}

/// The I/O a resource lends its prepared artifacts: the shared downloader, the
/// headers the audio source is read with, and the load's own cancel token.
///
/// Built from a [`ResourceConfig`](crate::ResourceConfig), so an artifact load
/// rides the same transport, the same cancel epoch, and the same credentials
/// policy as the audio it belongs to.
pub struct ArtifactFetch<'a> {
    audio: &'a ResourceSrc,
    cancel: Option<&'a CancelToken>,
    downloader: Option<&'a Downloader>,
    headers: Option<&'a Headers>,
}

impl<'a> ArtifactFetch<'a> {
    #[must_use]
    pub const fn new(
        audio: &'a ResourceSrc,
        downloader: Option<&'a Downloader>,
        headers: Option<&'a Headers>,
        cancel: Option<&'a CancelToken>,
    ) -> Self {
        Self {
            audio,
            cancel,
            downloader,
            headers,
        }
    }

    /// Headers to send for `url`. The audio source's credentials are the
    /// audio host's: an artifact served from anywhere else is fetched plain,
    /// so configuring an artifact URL can never leak a token to a third host.
    pub(super) fn headers_for(&self, url: &Url) -> Option<Headers> {
        let ResourceSrc::Url(audio) = self.audio else {
            return None;
        };
        (audio.host_str() == url.host_str() && audio.scheme() == url.scheme())
            .then(|| self.headers.cloned())
            .flatten()
    }

    /// Read one artifact document from `src`.
    ///
    /// # Errors
    ///
    /// Returns why the document could not be read or did not parse.
    pub async fn load<T: ArtifactDocument>(
        &self,
        src: &ResourceSrc,
    ) -> Result<Arc<T>, ArtifactLoadError> {
        if self.cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(ArtifactLoadError::Cancelled { kind: T::KIND });
        }
        let bytes = match src {
            ResourceSrc::Path(path) => self.read_file::<T>(path.clone()).await?,
            ResourceSrc::Url(url) => self.read_url::<T>(url).await?,
        };
        T::decode(&bytes)
            .map(Arc::new)
            .map_err(|reason| ArtifactLoadError::Decode {
                reason,
                kind: T::KIND,
            })
    }

    async fn read_file<T: ArtifactDocument>(
        &self,
        path: PathBuf,
    ) -> Result<Vec<u8>, ArtifactLoadError> {
        let display = path.display().to_string();
        let read = task::spawn_blocking(move || read_capped(&path))
            .await
            .map_err(|_| ArtifactLoadError::Cancelled { kind: T::KIND })?;
        match read {
            Ok(Capped::Bytes(bytes)) => Ok(bytes),
            Ok(Capped::TooLarge { bytes }) => Err(ArtifactLoadError::TooLarge {
                bytes,
                kind: T::KIND,
                limit: MAX_ARTIFACT_BYTES,
            }),
            Err(error) => Err(ArtifactLoadError::Fetch {
                kind: T::KIND,
                src: display,
                reason: error.to_string(),
            }),
        }
    }

    async fn read_url<T: ArtifactDocument>(&self, url: &Url) -> Result<Vec<u8>, ArtifactLoadError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ArtifactLoadError::Scheme {
                kind: T::KIND,
                scheme: url.scheme().to_owned(),
            });
        }
        let downloader = self
            .downloader
            .ok_or_else(|| ArtifactLoadError::NoDownloader {
                kind: T::KIND,
                url: url.clone(),
            })?;
        let scope = CancelScope::new(self.cancel.cloned());
        let handle = downloader.register(Arc::new(ArtifactPeer {
            cancel: scope.token(),
        }));
        let cmd = FetchCmd::get(url.clone())
            .cancel(scope.token())
            .maybe_headers(self.headers_for(url))
            .validator(reject_oversized_artifact)
            .build();
        match handle.execute(cmd).await {
            Ok(response) => response
                .body
                .collect()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| ArtifactLoadError::Fetch {
                    kind: T::KIND,
                    src: url.to_string(),
                    reason: error.to_string(),
                }),
            Err(NetError::Cancelled) => Err(ArtifactLoadError::Cancelled { kind: T::KIND }),
            Err(error) => Err(ArtifactLoadError::Fetch {
                kind: T::KIND,
                src: url.to_string(),
                reason: error.to_string(),
            }),
        }
    }
}

/// Refuse a response whose declared length is past the artifact cap, before
/// any of its body is buffered.
fn reject_oversized_artifact(headers: &Headers) -> NetResult<()> {
    let declared = headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    match declared {
        Some(bytes) if bytes > MAX_ARTIFACT_BYTES as u64 => Err(NetError::InvalidContentType(
            format!("artifact document of {bytes} bytes"),
        )),
        _ => Ok(()),
    }
}

/// Outcome of reading a local document under the artifact cap.
enum Capped {
    Bytes(Vec<u8>),
    TooLarge { bytes: u64 },
}

/// Read at most [`MAX_ARTIFACT_BYTES`] from `path`, reporting a larger file
/// rather than buffering it.
fn read_capped(path: &PathBuf) -> std::io::Result<Capped> {
    let file = File::open(path)?;
    let declared = file.metadata()?.len();
    if declared > MAX_ARTIFACT_BYTES as u64 {
        return Ok(Capped::TooLarge { bytes: declared });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(declared).unwrap_or(0));
    file.take(MAX_ARTIFACT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Ok(Capped::TooLarge {
            bytes: bytes.len() as u64,
        });
    }
    Ok(Capped::Bytes(bytes))
}

/// The downloader peer one artifact document is read over. It has no variants
/// and no buffer to report: it exists so the fetch rides the shared client and
/// dies with the load's cancel epoch.
struct ArtifactPeer {
    cancel: CancelToken,
}

impl Abr for ArtifactPeer {
    fn cancel(&self) -> CancelToken {
        self.cancel.clone()
    }
}

impl Peer for ArtifactPeer {}
