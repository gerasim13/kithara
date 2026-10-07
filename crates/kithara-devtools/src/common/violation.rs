use std::borrow::Cow;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Deserialize, derive_more::Display, Eq, PartialEq, Ord, PartialOrd, Serialize,
)]
pub enum Severity {
    #[display("WARN")]
    Warn,
    #[display("DENY")]
    Deny,
}

#[derive(Clone, Debug, Deserialize, fieldwork::Fieldwork, Serialize)]
#[fieldwork(opt_in, with)]
pub struct Violation {
    /// The check that found this. A kept verdict is read back by the check
    /// that reached it, which names itself again, so it is not stored.
    #[serde(skip)]
    pub check: &'static str,
    pub severity: Severity,
    pub key: String,
    pub message: String,
    /// Optional long-form explanation (Summary / Why / Bad / Good /
    /// Suppress block) shown in `--verbose` output and markdown reports.
    /// `None` keeps the compact one-line render.
    #[field(with, option_set_some)]
    pub(crate) explanation: Option<Cow<'static, str>>,
}

impl Violation {
    #[must_use]
    pub fn deny<K, M>(check: &'static str, key: K, message: M) -> Self
    where
        K: Into<String>,
        M: Into<String>,
    {
        build(Severity::Deny, check, key, message)
    }

    #[must_use]
    pub fn warn<K, M>(check: &'static str, key: K, message: M) -> Self
    where
        K: Into<String>,
        M: Into<String>,
    {
        build(Severity::Warn, check, key, message)
    }
}

/// Common builder used by both `Violation::deny` and `Violation::warn` to keep
/// their bodies DRY without exposing a third public-facing constructor on
/// `impl Violation` (the `multi_constructor` arch lint allows only one
/// canonical `new`/`default`).
fn build(
    severity: Severity,
    check: &'static str,
    key: impl Into<String>,
    message: impl Into<String>,
) -> Violation {
    Violation {
        check,
        severity,
        key: key.into(),
        message: message.into(),
        explanation: None,
    }
}

#[derive(Default)]
pub struct Report {
    pub violations: Vec<Violation>,
}

impl Report {
    #[must_use]
    pub fn deny_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Deny)
            .count()
    }

    pub fn extend<I>(&mut self, vs: I)
    where
        I: IntoIterator<Item = Violation>,
    {
        self.violations.extend(vs);
    }

    #[must_use]
    pub fn warn_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Warn)
            .count()
    }
}
