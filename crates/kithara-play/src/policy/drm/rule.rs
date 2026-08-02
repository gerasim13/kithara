use std::{collections::HashMap, fmt};

use kithara_drm::{KeyRequestFactory, PreparedKeyRequest};
use kithara_net::Headers;
use url::Url;

use crate::policy::domain::DomainPattern;

/// Domain rule for preparing provider-specific DRM key requests.
#[derive(Clone)]
#[non_exhaustive]
pub struct DomainKeyRule {
    key_request_factory: KeyRequestFactory,
    headers: Option<HashMap<String, String>>,
    query_params: Option<HashMap<String, String>>,
    domains: Vec<DomainPattern>,
}

impl fmt::Debug for DomainKeyRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainKeyRule")
            .field("domains", &self.domains)
            .field("header_names", &sorted_names(self.headers.as_ref()))
            .field("key_request_factory", &"<fn>")
            .field(
                "query_parameter_names",
                &sorted_names(self.query_params.as_ref()),
            )
            .finish_non_exhaustive()
    }
}

impl DomainKeyRule {
    /// Create a rule with no static headers or query parameters.
    ///
    /// Patterns use the same case-insensitive exact, `"*.example.com"`, and
    /// `"*"` forms as [`Self::for_domains`].
    #[must_use]
    pub fn new<P, I>(patterns: I, factory: KeyRequestFactory) -> Self
    where
        P: AsRef<str>,
        I: IntoIterator<Item = P>,
    {
        Self::for_domains(patterns, factory).build()
    }

    /// Start a rule builder with its domain patterns and request factory set.
    ///
    /// A bare pattern matches exactly, `"*.example.com"` matches subdomains
    /// but not the bare host, and `"*"` matches every host.
    pub fn for_domains<P, I>(patterns: I, factory: KeyRequestFactory) -> DomainKeyRuleBuilder
    where
        P: AsRef<str>,
        I: IntoIterator<Item = P>,
    {
        DomainKeyRuleBuilder {
            rule: Self {
                domains: patterns
                    .into_iter()
                    .map(|pattern| DomainPattern::parse(pattern.as_ref()))
                    .collect(),
                headers: None,
                key_request_factory: factory,
                query_params: None,
            },
        }
    }

    pub(super) fn matches(&self, url: &Url) -> bool {
        url.host_str()
            .is_some_and(|host| self.domains.iter().any(|domain| domain.matches(host)))
    }

    pub(super) fn prepare(&self, key_url: &Url) -> PreparedKeyRequest {
        let request = (self.key_request_factory)();
        let mut headers = self.headers.clone().unwrap_or_default();
        headers.extend(request.headers);

        let mut url = key_url.clone();
        if let Some(query_params) = self.query_params.as_ref() {
            let mut query_params: Vec<_> = query_params.iter().collect();
            query_params.sort_unstable_by_key(|(key, _)| *key);
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query_params {
                pairs.append_pair(key, value);
            }
        }

        PreparedKeyRequest::new(url, headers, request.processor)
    }

    pub(super) fn resource_headers(&self) -> Option<Headers> {
        self.headers
            .as_ref()
            .filter(|headers| !headers.is_empty())
            .cloned()
            .map(Headers::from)
    }
}

/// Builder for optional provider request fields.
#[must_use]
#[non_exhaustive]
pub struct DomainKeyRuleBuilder {
    rule: DomainKeyRule,
}

impl fmt::Debug for DomainKeyRuleBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainKeyRuleBuilder")
            .field("rule", &self.rule)
            .finish_non_exhaustive()
    }
}

impl DomainKeyRuleBuilder {
    /// Finish the rule.
    #[must_use]
    pub fn build(self) -> DomainKeyRule {
        self.rule
    }

    /// Set static headers added before the per-request factory headers.
    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        self.rule.headers = Some(headers);
        self
    }

    /// Set static headers when present.
    pub fn maybe_headers(mut self, headers: Option<HashMap<String, String>>) -> Self {
        self.rule.headers = headers;
        self
    }

    /// Set query parameters when present.
    pub fn maybe_query_params(mut self, query_params: Option<HashMap<String, String>>) -> Self {
        self.rule.query_params = query_params;
        self
    }

    /// Set query parameters appended to matching key URLs.
    pub fn query_params(mut self, query_params: HashMap<String, String>) -> Self {
        self.rule.query_params = Some(query_params);
        self
    }
}

fn sorted_names(values: Option<&HashMap<String, String>>) -> Vec<&str> {
    let mut names: Vec<_> = values
        .into_iter()
        .flat_map(HashMap::keys)
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names
}
