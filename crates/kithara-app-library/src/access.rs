use std::fmt;

use kithara_platform::tokio::sync::watch;

/// An account's access token. `Debug` hides the value.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    /// The token as sent on the wire.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

/// A plugin's grant: while the plugin holds a token, key requests to `domain`
/// carry `header` set to it.
#[derive(Clone)]
pub struct KeyAccess {
    domain: &'static str,
    header: &'static str,
    token: watch::Receiver<Option<AccessToken>>,
}

impl KeyAccess {
    #[must_use]
    pub const fn new(
        domain: &'static str,
        header: &'static str,
        token: watch::Receiver<Option<AccessToken>>,
    ) -> Self {
        Self {
            domain,
            header,
            token,
        }
    }

    #[must_use]
    pub const fn domain(&self) -> &'static str {
        self.domain
    }

    #[must_use]
    pub const fn header(&self) -> &'static str {
        self.header
    }

    /// The token as sent on the wire at the time of the call.
    #[must_use]
    pub fn token(&self) -> Option<String> {
        self.token
            .borrow()
            .as_ref()
            .map(|token| token.expose().to_owned())
    }
}

impl fmt::Debug for KeyAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyAccess")
            .field("domain", &self.domain)
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    mod consts {
        pub(super) const TOKEN: &str = "synthetic-token-value";
    }

    #[kithara::test]
    fn neither_a_token_nor_a_grant_shows_the_token() {
        let token = AccessToken::new(consts::TOKEN.to_owned());
        let (_sender, receiver) = watch::channel(Some(token.clone()));
        let access = KeyAccess::new("example.com", "X-Auth-Token", receiver);

        let shown = format!("{token:?} {access:?}");

        assert!(!shown.contains(consts::TOKEN), "{shown}");
        assert!(shown.contains("example.com"), "{shown}");
    }
}
