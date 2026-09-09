use std::fmt;

use anyhow::Error;

/// A child-command failure that should be reported without a wrapper backtrace.
#[derive(Debug)]
#[non_exhaustive]
pub struct ChildFailure {
    exit_code: Option<i32>,
    stderr: Option<String>,
    label: String,
}

impl fmt::Display for ChildFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed", self.label)?;
        match self.exit_code {
            Some(code) => write!(f, " (exit code {code})")?,
            None => write!(f, " (terminated without an exit code)")?,
        }
        if let Some(stderr) = &self.stderr {
            write!(f, ": {stderr}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ChildFailure {}

impl ChildFailure {
    /// Report a child whose standard error was captured by the current process.
    #[must_use]
    pub fn captured(label: String, exit_code: Option<i32>, stderr: String) -> Error {
        let stderr = (!stderr.is_empty()).then_some(stderr);
        Error::new(Self {
            exit_code,
            stderr,
            label,
        })
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.exit_code
            .filter(|code| (1..=i32::from(u8::MAX)).contains(code))
            .unwrap_or(1)
    }

    /// Report a child whose output was inherited by the current process.
    #[must_use]
    pub fn inherited(label: String, exit_code: Option<i32>) -> Error {
        Error::new(Self {
            label,
            exit_code,
            stderr: None,
        })
    }
}

/// A check ran to completion and did not pass.
///
/// This is not an error: nothing went wrong with the tool, and there is no
/// state to inspect. Carried as one so it can travel the same `Result` as a
/// real failure, and recognised at the top so the two print differently - a
/// failure earns a backtrace, a verdict earns a sentence. Twenty-three frames
/// of runtime internals under "the code has ten style violations" says the
/// program broke, which is the one thing that did not happen.
#[derive(Debug)]
pub struct NotClean {
    /// The check that reached the verdict, as the user invoked it.
    pub check: &'static str,
    /// What it found, already printed above in full. `None` when the check
    /// reports only that the check did not pass, without a count.
    pub findings: Option<usize>,
}

impl fmt::Display for NotClean {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.findings {
            Some(findings) => write!(
                f,
                "{check}: {findings} finding{plural} above. The check ran; the \
                 check did not pass.",
                check = self.check,
                plural = if findings == 1 { "" } else { "s" },
            ),
            None => write!(
                f,
                "{check}: the check ran and did not pass. Its \
                 findings are above.",
                check = self.check,
            ),
        }
    }
}

impl std::error::Error for NotClean {}

impl NotClean {
    /// Not `new`: this hands back the `anyhow::Error` a check returns, not the
    /// verdict itself, and a `new` that does not return `Self` reads wrong.
    #[must_use]
    pub fn raised(check: &'static str, findings: usize) -> Error {
        Error::new(Self {
            check,
            findings: Some(findings),
        })
    }

    fn render(error: &Error) -> (String, i32) {
        if let Some(verdict) = error.downcast_ref::<Self>() {
            return (verdict.to_string(), 1);
        }

        if let Some(failure) = error.downcast_ref::<ChildFailure>() {
            return (failure.to_string(), failure.exit_code());
        }

        (format!("Error: {error:?}"), 1)
    }

    /// Print `error` the way it deserves and give the exit code to leave with.
    ///
    /// Verdicts and reported child failures print their sentence. Anything
    /// else is a genuine tool failure and keeps its chain and backtrace.
    #[must_use]
    pub fn report(error: &Error) -> i32 {
        let (message, code) = Self::render(error);
        eprintln!("{message}");
        code
    }

    /// The same verdict from a check that prints its findings but does not
    /// hand back a count.
    #[must_use]
    pub fn reported(check: &'static str) -> Error {
        Error::new(Self {
            check,
            findings: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_says_what_ran_and_what_it_found() {
        let verdict = NotClean {
            check: "ast-grep",
            findings: Some(10),
        };
        let text = verdict.to_string();
        assert!(text.starts_with("ast-grep: 10 findings"), "{text}");
        assert!(
            !text.contains("failed"),
            "a verdict must not read as a failure: {text}"
        );
    }

    #[test]
    fn one_finding_is_not_pluralised() {
        let verdict = NotClean {
            check: "ast-grep",
            findings: Some(1),
        };
        assert!(verdict.to_string().contains("1 finding above"), "{verdict}");
    }

    #[test]
    fn a_check_without_a_count_still_reads_as_a_verdict() {
        let error = NotClean::reported("typos");
        let text = error.to_string();
        assert!(text.starts_with("typos: the check ran"), "{text}");
        assert!(!text.contains("failed"), "{text}");
        assert_eq!(NotClean::render(&error), (text, 1));
    }

    /// The distinction is the whole point: a verdict is recognised through the
    /// same `anyhow::Error` a real failure travels in.
    #[test]
    fn a_real_failure_is_not_mistaken_for_a_verdict() {
        let verdict = NotClean::raised("ast-grep", 3);
        assert!(verdict.downcast_ref::<NotClean>().is_some());

        let failure = anyhow::anyhow!("ast-grep is not installed");
        assert!(failure.downcast_ref::<NotClean>().is_none());
        assert!(failure.downcast_ref::<ChildFailure>().is_none());
    }

    #[test]
    fn a_child_failure_is_classified_separately() {
        let error = ChildFailure::inherited("test lane `workspace`".to_owned(), Some(100));

        assert!(error.downcast_ref::<ChildFailure>().is_some());
        assert!(error.downcast_ref::<NotClean>().is_none());
        assert_eq!(
            error.to_string(),
            "test lane `workspace` failed (exit code 100)"
        );
    }

    #[test]
    fn a_child_failure_preserves_a_representable_exit_code() {
        let error = ChildFailure::inherited("test lane `workspace`".to_owned(), Some(100));
        let (message, code) = NotClean::render(&error);

        assert_eq!(code, 100);
        assert_eq!(message, "test lane `workspace` failed (exit code 100)");
        assert!(!message.contains("Stack backtrace"));
        assert!(!message.contains("kithara_devtools::test"));
        assert_eq!(message.lines().count(), 1);
    }

    #[test]
    fn a_context_wrapped_child_failure_remains_concise_and_typed() {
        let error = ChildFailure::inherited("`cargo fmt --all`".to_owned(), Some(7))
            .context("format target `rust`");
        let (message, code) = NotClean::render(&error);

        assert!(error.downcast_ref::<ChildFailure>().is_some());
        assert_eq!(code, 7);
        assert_eq!(message, "`cargo fmt --all` failed (exit code 7)");
        assert_eq!(message.lines().count(), 1);
        assert!(!message.contains("format target"));
        assert!(!message.contains("Stack backtrace"));
    }

    #[test]
    fn a_child_without_an_exit_code_uses_the_generic_failure_code() {
        let error = ChildFailure::inherited("test lane `workspace`".to_owned(), None);
        let (message, code) = NotClean::render(&error);

        assert_eq!(code, 1);
        assert_eq!(
            message,
            "test lane `workspace` failed (terminated without an exit code)"
        );
    }

    #[test]
    fn an_unrepresentable_child_exit_code_uses_the_generic_failure_code() {
        for exit_code in [0, i32::from(u8::MAX) + 1, -1] {
            let error = ChildFailure::inherited("fixture command".to_owned(), Some(exit_code));

            assert_eq!(NotClean::render(&error).1, 1);
        }
    }

    #[test]
    fn a_captured_child_failure_keeps_its_detail() {
        let error = ChildFailure::captured(
            "git status".to_owned(),
            Some(7),
            "repository unavailable".to_owned(),
        );
        let failure = error
            .downcast_ref::<ChildFailure>()
            .expect("typed child failure");

        assert_eq!(
            failure.to_string(),
            "git status failed (exit code 7): repository unavailable"
        );
    }

    #[test]
    fn a_genuine_failure_keeps_debug_rendering() {
        let error = anyhow::anyhow!("invalid internal state").context("tool failed");
        let expected = format!("Error: {error:?}");

        assert_eq!(NotClean::render(&error), (expected, 1));
    }
}
