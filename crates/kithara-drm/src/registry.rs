use std::{collections::HashMap, fmt};

use bytes::Bytes;
use kithara_platform::sync::Arc;
use url::Url;

/// Result of processing a key through a [`KeyProcessor`].
pub type KeyProcessResult = Result<Bytes, crate::DrmError>;

/// Callback that transforms raw key bytes fetched from the server.
pub type KeyProcessor = Arc<dyn Fn(Bytes) -> KeyProcessResult + Send + Sync>;

/// Factory that produces fresh request-local headers and their matching
/// processor for one key fetch.
pub type KeyRequestFactory = Arc<dyn Fn() -> KeyRequest + Send + Sync>;

/// Request-local headers and the processor paired with their response.
#[non_exhaustive]
pub struct KeyRequest {
    /// Headers added to the outgoing key request.
    pub headers: HashMap<String, String>,
    /// Processor paired with the response to this request.
    pub processor: KeyProcessor,
}

impl KeyRequest {
    /// Construct request-local headers and their paired processor.
    #[must_use]
    pub fn new(headers: HashMap<String, String>, processor: KeyProcessor) -> Self {
        Self { headers, processor }
    }
}

/// Final wire request prepared by a [`KeyRequestResolver`].
///
/// The URL and headers may contain authentication material, so the
/// [`Debug`](fmt::Debug) implementation redacts every field.
#[non_exhaustive]
pub struct PreparedKeyRequest {
    /// Policy-specific headers merged on top of the downloader headers.
    pub headers: HashMap<String, String>,
    /// Processor paired with this request's response.
    pub processor: KeyProcessor,
    /// Final URL sent over the network.
    pub url: Url,
}

impl fmt::Debug for PreparedKeyRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedKeyRequest")
            .field("url", &"<redacted>")
            .field("headers", &"<redacted>")
            .field("processor", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl PreparedKeyRequest {
    /// Construct a final request from its wire URL, policy headers,
    /// and paired response processor.
    #[must_use]
    pub fn new(url: Url, headers: HashMap<String, String>, processor: KeyProcessor) -> Self {
        Self {
            headers,
            processor,
            url,
        }
    }
}

/// Prepares a wire request or declines a key URL for the plain AES-128 path.
/// A result must pair fresh request-local material with its processor.
pub trait KeyRequestResolver: Send + Sync {
    /// Prepare a final wire request, or decline this URL.
    fn prepare(&self, key_url: &Url) -> Option<PreparedKeyRequest>;
}

/// Ordered registry of key-request resolvers.
///
/// Resolvers are consulted in registration order. The first resolver that
/// prepares a request wins.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct KeyProcessorRegistry {
    resolvers: Vec<Arc<dyn KeyRequestResolver>>,
}

impl fmt::Debug for KeyProcessorRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyProcessorRegistry")
            .field("resolver_count", &self.resolvers.len())
            .finish_non_exhaustive()
    }
}

impl KeyProcessorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the registry has any resolvers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resolvers.is_empty()
    }

    /// Ask resolvers in registration order to prepare this request.
    #[must_use]
    pub fn prepare(&self, key_url: &Url) -> Option<PreparedKeyRequest> {
        self.resolvers
            .iter()
            .find_map(|resolver| resolver.prepare(key_url))
    }

    /// Append a resolver to the registry.
    pub fn register(&mut self, resolver: Arc<dyn KeyRequestResolver>) -> &mut Self {
        self.resolvers.push(resolver);
        self
    }
}
