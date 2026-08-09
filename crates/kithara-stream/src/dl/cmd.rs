use std::io;

use bon::Builder;
use kithara_events::{RequestMethod, RequestPriority};
use kithara_net::{Headers, NetError, NetResult, RangeSpec};
use kithara_platform::CancelToken;
use url::Url;

/// Per-command body writer. Downloader calls it for each chunk.
pub type WriterFn = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

/// Per-command response callback. Fires once on the streaming path
/// when the HTTP response is in hand — past validation, headers
/// available, body about to stream. Mirrors [`OnCompleteFn`] on the
/// other end of the fetch lifecycle: peers use it to seed metadata
/// (Content-Length, Content-Type) eagerly so a reader blocked on the
/// first byte already sees a populated coord.
pub type OnResponseFn = Box<dyn FnOnce(&Headers) + Send>;

/// Per-command completion handler. Called when the fetch completes.
///
/// Receives `(bytes_written, response_headers, error)`. Headers are
/// `Some` once the HTTP response made it past validation (so `Content-Type`,
/// `Content-Length`, etc. can be captured); `None` when the fetch failed
/// before headers were received.
pub type OnCompleteFn = Box<dyn FnOnce(u64, Option<&Headers>, Option<&NetError>) + Send>;

/// Per-command slow-fetch hook. Fires once when the fetch outlasts
/// `DownloaderConfig::soft_timeout` without completing — the twin of
/// [`OnCompleteFn`] for the mid-flight "still pending, now slow"
/// transition. The request keeps running; this is advisory, fired at the
/// same point that publishes [`DownloaderEvent::LoadSlow`](kithara_events::DownloaderEvent::LoadSlow).
pub type OnSlowFn = Box<dyn FnOnce() + Send>;

/// Optional response-header validator for a single `FetchCmd`.
///
/// Invoked with the response headers after a successful HTTP response.
/// Returning `Err` rejects the response before the body is consumed.
pub(super) type ResponseValidator = fn(&Headers) -> NetResult<()>;

/// A single download command.
///
/// Built by protocol code via [`FetchCmd::get`] / [`FetchCmd::head`]
/// constructors and executed via
/// [`PeerHandle::execute`](super::PeerHandle::execute). The downloader
/// establishes the HTTP connection and returns a
/// [`FetchResponse`](super::FetchResponse) with headers and a body stream.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct FetchCmd {
    /// URL to fetch.
    #[builder(start_fn)]
    pub(crate) url: Url,
    /// Epoch cancel token from the Peer. When set, the Downloader
    /// combines it with the track-level cancel via [`CancelGroup`].
    pub(crate) cancel: Option<CancelToken>,
    /// Additional HTTP headers for this request.
    pub(crate) headers: Option<Headers>,
    /// Streaming path completion handler. `None` for channel path (`execute`/`batch`).
    pub(crate) on_complete: Option<OnCompleteFn>,
    /// Streaming path response callback — fires once when the
    /// response is ready, before the body streams. `None` for the
    /// channel path (`execute`/`batch`).
    pub(crate) on_response: Option<OnResponseFn>,
    /// Streaming path slow hook — fires once at `soft_timeout` if the
    /// fetch has not completed. `None` for callers that don't observe
    /// slowness. The request keeps running regardless.
    pub(crate) on_slow: Option<OnSlowFn>,
    /// Scheduling priority for proactive peer fetches.
    pub(crate) priority: Option<RequestPriority>,
    /// Optional byte range (HTTP Range request).
    pub(crate) range: Option<RangeSpec>,
    /// Optional per-request response validator.
    /// Called with the response headers after a successful HTTP response.
    /// Return `Err` to reject the response before the body is consumed.
    pub(crate) validator: Option<ResponseValidator>,
    /// Streaming path body writer. `None` for channel path (`execute`/`batch`).
    pub(crate) writer: Option<WriterFn>,
    /// HTTP method.
    pub(crate) method: RequestMethod,
}

impl FetchCmd {
    /// Builder for an HTTP GET command targeting the given URL.
    pub fn get(url: Url) -> FetchCmdBuilder<fetch_cmd_builder::SetMethod> {
        Self::builder(url).method(RequestMethod::Get)
    }

    /// Builder for an HTTP HEAD command targeting the given URL.
    pub fn head(url: Url) -> FetchCmdBuilder<fetch_cmd_builder::SetMethod> {
        Self::builder(url).method(RequestMethod::Head)
    }

    /// Escalate an already-built command's scheduling priority (e.g. a
    /// decoder-blocking init/segment fetch promoted after the peer decides
    /// it is owed urgent service).
    pub fn set_priority(&mut self, priority: RequestPriority) {
        self.priority = Some(priority);
    }

    /// Scheduling priority for proactive peer fetches.
    #[must_use]
    pub fn priority(&self) -> Option<RequestPriority> {
        self.priority
    }

    /// URL this command fetches.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Epoch cancel token carried by this command, if any.
    #[must_use]
    pub fn cancel(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    /// Take the streaming-path body writer, leaving `None` in its place.
    pub fn take_writer(&mut self) -> Option<WriterFn> {
        self.writer.take()
    }

    /// Take the streaming-path completion handler, leaving `None` in its place.
    pub fn take_on_complete(&mut self) -> Option<OnCompleteFn> {
        self.on_complete.take()
    }
}

impl std::fmt::Debug for FetchCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchCmd")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}

/// Reject responses with `content-type: text/html`.
///
/// Protects against CDN soft-error pages that return `200 OK` with an HTML
/// body. Pass as the validator argument to [`FetchCmd::validator`].
///
/// # Errors
///
/// Returns [`NetError::InvalidContentType`] when the response `content-type`
/// header starts with `text/html`.
pub fn reject_html_response(headers: &Headers) -> NetResult<()> {
    if let Some(ct) = headers.get("content-type")
        && ct.starts_with("text/html")
    {
        return Err(NetError::InvalidContentType(ct.to_string()));
    }
    Ok(())
}
