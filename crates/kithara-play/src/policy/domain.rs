#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DomainPattern {
    All,
    Exact(String),
    Wildcard(String),
}

impl DomainPattern {
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
}
