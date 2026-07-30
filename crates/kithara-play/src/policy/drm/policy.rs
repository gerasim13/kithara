use std::fmt;

use kithara_drm::{KeyRequestResolver, PreparedKeyRequest};
use kithara_net::Headers;
use url::Url;

use super::rule::DomainKeyRule;

/// Ordered domain policy used as one ordinary DRM registry resolver.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct DomainKeyPolicy {
    rules: Vec<DomainKeyRule>,
}

impl fmt::Debug for DomainKeyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainKeyPolicy")
            .field("rules", &self.rules)
            .finish_non_exhaustive()
    }
}

impl DomainKeyPolicy {
    /// Create a policy whose first matching rule wins.
    #[must_use]
    pub fn new<I>(rules: I) -> Self
    where
        I: IntoIterator<Item = DomainKeyRule>,
    {
        Self {
            rules: rules.into_iter().collect(),
        }
    }

    /// Return static resource headers from the first matching rule.
    #[must_use]
    pub fn resource_headers(&self, url: &Url) -> Option<Headers> {
        self.rule(url).and_then(DomainKeyRule::resource_headers)
    }

    fn rule(&self, url: &Url) -> Option<&DomainKeyRule> {
        self.rules.iter().find(|rule| rule.matches(url))
    }
}

impl KeyRequestResolver for DomainKeyPolicy {
    fn prepare(&self, key_url: &Url) -> Option<PreparedKeyRequest> {
        self.rule(key_url).map(|rule| rule.prepare(key_url))
    }
}
