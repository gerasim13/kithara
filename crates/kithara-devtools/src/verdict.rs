use std::fmt;

/// A check ran to completion and the code did not pass it.
///
/// This is not an error: nothing went wrong with the tool, and there is no
/// state to inspect. Carried as one so it can travel the same `Result` as a
/// real failure, and recognised at the top so the two print differently — a
/// failure earns a backtrace, a verdict earns a sentence. Twenty-three frames
/// of runtime internals under "the code has ten style violations" says the
/// program broke, which is the one thing that did not happen.
#[derive(Debug)]
pub struct NotClean {
    /// The check that reached the verdict, as the user invoked it.
    pub check: &'static str,
    /// What it found, already printed above in full.
    pub findings: usize,
}

impl fmt::Display for NotClean {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{check}: {findings} finding{plural} above. The check ran; the code \
             did not pass it.",
            check = self.check,
            findings = self.findings,
            plural = if self.findings == 1 { "" } else { "s" },
        )
    }
}

impl std::error::Error for NotClean {}

impl NotClean {
    /// Not `new`: this hands back the `anyhow::Error` a check returns, not the
    /// verdict itself, and a `new` that does not return `Self` reads wrong.
    #[must_use]
    pub fn raised(check: &'static str, findings: usize) -> anyhow::Error {
        anyhow::Error::new(Self { check, findings })
    }

    /// Print `error` the way it deserves and give the exit code to leave with.
    ///
    /// A verdict prints its sentence. Anything else is a genuine failure and
    /// keeps the full chain and backtrace it would have had.
    #[must_use]
    pub fn report(error: &anyhow::Error) -> i32 {
        if let Some(verdict) = error.downcast_ref::<Self>() {
            eprintln!("{verdict}");
        } else {
            eprintln!("Error: {error:?}");
        }
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_says_what_ran_and_what_it_found() {
        let verdict = NotClean {
            check: "ast-grep",
            findings: 10,
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
            findings: 1,
        };
        assert!(verdict.to_string().contains("1 finding above"), "{verdict}");
    }

    /// The distinction is the whole point: a verdict is recognised through the
    /// same `anyhow::Error` a real failure travels in.
    #[test]
    fn a_real_failure_is_not_mistaken_for_a_verdict() {
        let verdict = NotClean::raised("ast-grep", 3);
        assert!(verdict.downcast_ref::<NotClean>().is_some());

        let failure = anyhow::anyhow!("ast-grep is not installed");
        assert!(failure.downcast_ref::<NotClean>().is_none());
    }
}
