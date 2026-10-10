#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DomainPattern {
    All,
    Exact(String),
    Wildcard(String),
}

impl DomainPattern {
    pub(super) fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match self {
            Self::All => true,
            Self::Exact(domain) => host == *domain,
            Self::Wildcard(suffix) => host
                .strip_suffix(suffix)
                .is_some_and(|prefix| !prefix.is_empty() && prefix.ends_with('.')),
        }
    }

    pub(super) fn parse(pattern: &str) -> Self {
        let pattern = pattern.to_ascii_lowercase();
        if pattern == "*" {
            return Self::All;
        }
        pattern.strip_prefix("*.").map_or_else(
            || Self::Exact(pattern.clone()),
            |suffix| Self::Wildcard(suffix.to_string()),
        )
    }
}

/// Whether `host` falls under `pattern`, read the way policy rules read their
/// domains: a bare host matches exactly, `*.domain` its subdomains, `*` any.
#[must_use]
pub fn domain_matches(pattern: &str, host: &str) -> bool {
    DomainPattern::parse(pattern).matches(host)
}

/// Whether every host `pattern` matches is `domain` or one of its subdomains.
#[must_use]
pub fn domain_holds(domain: &str, pattern: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    let subdomains = DomainPattern::Wildcard(domain.clone());
    match DomainPattern::parse(pattern) {
        DomainPattern::All => false,
        DomainPattern::Exact(host) | DomainPattern::Wildcard(host) => {
            host == domain || subdomains.matches(&host)
        }
    }
}
