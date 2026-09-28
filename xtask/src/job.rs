//! Which CI job this process runs as.

use std::{env, process};

use anyhow::{Context, Result};

/// Whether this process runs inside a GitLab CI job.
pub(crate) fn is_gitlab() -> bool {
    gitlab_in(&|name| env::var(name).ok())
}

fn gitlab_in(var: &dyn Fn(&str) -> Option<String>) -> bool {
    var("GITLAB_CI").is_some_and(|value| !value.is_empty())
}

/// Names this process to a job that waits on a lock it holds: the link to
/// the CI job it runs in, or the local command that took the lock.
///
/// # Errors
///
/// When the CI provider's environment lacks a variable the link is built
/// from: a job that cannot be named runs in a broken environment.
pub(crate) fn lock_holder() -> Result<String> {
    holder_in(&|name| env::var(name).ok())
}

fn holder_in(var: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let required = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .with_context(|| format!("{name} is unset in a CI job"))
    };
    if var("GITHUB_ACTIONS").is_some_and(|value| value == "true") {
        return Ok(format!(
            "{}/{}/actions/runs/{}/attempts/{} on {}",
            required("GITHUB_SERVER_URL")?,
            required("GITHUB_REPOSITORY")?,
            required("GITHUB_RUN_ID")?,
            required("GITHUB_RUN_ATTEMPT")?,
            required("RUNNER_NAME")?,
        ));
    }
    if gitlab_in(var) {
        return required("CI_JOB_URL");
    }
    Ok(format!(
        "pid {} running {}",
        process::id(),
        env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use std::process;

    use super::*;

    fn environment<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn a_github_holder_links_the_run_attempt_and_names_the_runner() {
        let var = environment(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "zvuk/kithara"),
            ("GITHUB_RUN_ID", "77"),
            ("GITHUB_RUN_ATTEMPT", "2"),
            ("RUNNER_NAME", "kithara-3"),
        ]);

        assert_eq!(
            holder_in(&var).unwrap(),
            "https://github.com/zvuk/kithara/actions/runs/77/attempts/2 on kithara-3"
        );
    }

    #[test]
    fn a_gitlab_holder_is_the_job_link() {
        let var = environment(&[
            ("GITLAB_CI", "true"),
            ("CI_JOB_URL", "https://gitlab.example/-/jobs/29"),
        ]);

        assert_eq!(holder_in(&var).unwrap(), "https://gitlab.example/-/jobs/29");
    }

    #[test]
    fn a_ci_job_missing_what_names_it_is_a_broken_environment() {
        let github = environment(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "zvuk/kithara"),
            ("GITHUB_RUN_ID", "77"),
            ("GITHUB_RUN_ATTEMPT", "2"),
        ]);
        let gitlab = environment(&[("GITLAB_CI", "true"), ("CI_JOB_URL", "")]);

        let github = holder_in(&github).unwrap_err().to_string();
        let gitlab = holder_in(&gitlab).unwrap_err().to_string();

        assert!(github.contains("RUNNER_NAME"), "{github}");
        assert!(gitlab.contains("CI_JOB_URL"), "{gitlab}");
    }

    #[test]
    fn a_local_holder_names_its_process() {
        let holder = holder_in(&environment(&[])).unwrap();

        assert!(
            holder.starts_with(&format!("pid {} running ", process::id())),
            "{holder}"
        );
    }

    #[test]
    fn gitlab_is_named_by_a_non_empty_flag() {
        assert!(gitlab_in(&environment(&[("GITLAB_CI", "true")])));
        assert!(!gitlab_in(&environment(&[("GITLAB_CI", "")])));
        assert!(!gitlab_in(&environment(&[])));
    }
}
