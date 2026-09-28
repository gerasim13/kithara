//! Which CI job this process runs as.

use std::{env, process};

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
/// A variable the CI job left unset is named in its place as
/// `<NAME unset>`. The name only serves a waiter's log, and a job never
/// fails for its diagnostics.
pub(crate) fn lock_holder() -> String {
    holder_in(&|name| env::var(name).ok())
}

fn holder_in(var: &dyn Fn(&str) -> Option<String>) -> String {
    let named = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("<{name} unset>"))
    };
    if var("GITHUB_ACTIONS").is_some_and(|value| value == "true") {
        return format!(
            "{}/{}/actions/runs/{}/attempts/{} on {}",
            named("GITHUB_SERVER_URL"),
            named("GITHUB_REPOSITORY"),
            named("GITHUB_RUN_ID"),
            named("GITHUB_RUN_ATTEMPT"),
            named("RUNNER_NAME"),
        );
    }
    if gitlab_in(var) {
        return named("CI_JOB_URL");
    }
    format!(
        "pid {} running {}",
        process::id(),
        env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    )
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
            holder_in(&var),
            "https://github.com/zvuk/kithara/actions/runs/77/attempts/2 on kithara-3"
        );
    }

    #[test]
    fn a_gitlab_holder_is_the_job_link() {
        let var = environment(&[
            ("GITLAB_CI", "true"),
            ("CI_JOB_URL", "https://gitlab.example/-/jobs/29"),
        ]);

        assert_eq!(holder_in(&var), "https://gitlab.example/-/jobs/29");
    }

    /// Naming the holder serves a waiter's log: a variable the CI job left
    /// unset is named in its place, and the job that takes the lock works on.
    #[test]
    fn a_holder_names_in_place_what_its_ci_job_left_unset() {
        let github = environment(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "zvuk/kithara"),
            ("GITHUB_RUN_ID", "77"),
            ("GITHUB_RUN_ATTEMPT", "2"),
        ]);
        let gitlab = environment(&[("GITLAB_CI", "true"), ("CI_JOB_URL", "")]);

        assert_eq!(
            holder_in(&github),
            "https://github.com/zvuk/kithara/actions/runs/77/attempts/2 on <RUNNER_NAME unset>"
        );
        assert_eq!(holder_in(&gitlab), "<CI_JOB_URL unset>");
    }

    #[test]
    fn a_local_holder_names_its_process() {
        let holder = holder_in(&environment(&[]));

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
