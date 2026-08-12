use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use super::{
    api::{Github, Gitlab},
    command::BridgeConfig,
    model::CONTROL_PATHS,
};

pub(super) struct GitRepo {
    root: PathBuf,
    config: BridgeConfig,
}

impl GitRepo {
    pub(super) fn new(state_dir: &Path, config: &BridgeConfig) -> Result<Self> {
        let root = state_dir.join("repository.git");
        if !root.exists() {
            fs::create_dir_all(state_dir)
                .with_context(|| format!("creating bridge state {}", state_dir.display()))?;
            let output = Command::new("git")
                .current_dir(state_dir)
                .args(["init", "--bare"])
                .arg(&root)
                .output()
                .context("initializing bridge bare repository")?;
            checked(output, "git init --bare")?;
        }
        Ok(Self {
            root,
            config: config.clone(),
        })
    }

    pub(super) fn fetch(&self, github: &Github, gitlab: &Gitlab) -> Result<()> {
        let github_url = format!("https://github.com/{}.git", self.config.github_repo);
        self.run(
            &[
                "fetch",
                "--no-tags",
                "--force",
                &github_url,
                &format!(
                    "+refs/heads/{}:refs/heads/bridge/github",
                    self.config.branch
                ),
            ],
            Some(github.git_header()),
        )?;

        let gitlab_url = format!(
            "{}/{}.git",
            self.config.gitlab_origin(),
            self.config.gitlab_project_path
        );
        self.run(
            &[
                "fetch",
                "--no-tags",
                "--force",
                &gitlab_url,
                &format!(
                    "+refs/heads/{}:refs/heads/bridge/gitlab",
                    self.config.branch
                ),
            ],
            Some(gitlab.git_header()),
        )?;
        Ok(())
    }

    pub(super) fn is_ancestor(&self, older: &str, newer: &str) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["merge-base", "--is-ancestor", older, newer])
            .output()
            .context("running git merge-base --is-ancestor")?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            code => bail!(
                "git merge-base failed with exit code {}: {}",
                code.unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    /// The commit quarantine judges: everything the pull request changed, with
    /// every CI control path taken from the branch it is being imported into.
    ///
    /// A patch may not choose the pipeline that judges it. Refusing such a patch
    /// outright was the first answer and it does not survive contact: the advice
    /// it printed — land the change through a merge request here instead — makes
    /// the same content arrive as a different commit, and two branches holding
    /// the same work under different hashes are exactly the divergence this
    /// bridge cannot repair. Restoring the control paths keeps the judge
    /// trusted by construction, so nothing has to be refused.
    ///
    /// The control paths themselves therefore reach the branch unjudged, on the
    /// authority of the human who merged the pull request. The next pipeline is
    /// the first to run them, which is the same exposure as any change to CI
    /// made here directly.
    pub(super) fn judged_commit(&self, base: &str, head: &str) -> Result<String> {
        let tree = self.root.join("quarantine-worktree");
        self.discard_worktree(&tree)?;
        self.run(
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                path_text(&tree)?,
                head,
            ],
            None,
        )?;
        let outcome = self.restore_control_paths(&tree, base);
        let commit = outcome.and_then(|()| Self::commit_worktree(&tree, head));
        self.discard_worktree(&tree)?;
        commit
    }

    fn restore_control_paths(&self, tree: &Path, base: &str) -> Result<()> {
        for path in CONTROL_PATHS {
            let path = path.trim_end_matches('/');
            if self.exists_at(base, path)? {
                git_in(tree, &["checkout", base, "--", path])?;
            } else {
                // The pull request introduced it, so the trusted branch has
                // nothing to restore and the judged tree must not carry it.
                git_in(
                    tree,
                    &["rm", "-r", "--force", "--ignore-unmatch", "--", path],
                )?;
            }
        }
        Ok(())
    }

    fn commit_worktree(tree: &Path, head: &str) -> Result<String> {
        git_in(tree, &["add", "--all"])?;
        let message = format!("quarantine: {head} judged with this branch's CI");
        // An unchanged tree means the pull request touched no control path, and
        // then the head is already the commit to judge.
        if git_in(tree, &["diff", "--cached", "--quiet"]).is_ok() {
            return Ok(head.to_owned());
        }
        git_in(
            tree,
            &[
                "-c",
                "user.name=kithara-bridge",
                "-c",
                "user.email=kithara-bridge@localhost",
                "commit",
                "--quiet",
                "--message",
                &message,
            ],
        )?;
        let sha = git_in(tree, &["rev-parse", "HEAD"])?;
        Ok(String::from_utf8(sha)
            .context("git rev-parse returned invalid UTF-8")?
            .trim()
            .to_owned())
    }

    fn discard_worktree(&self, tree: &Path) -> Result<()> {
        if tree.exists() {
            let _ = self.run(&["worktree", "remove", "--force", path_text(tree)?], None);
            if tree.exists() {
                fs::remove_dir_all(tree).with_context(|| format!("removing {}", tree.display()))?;
            }
        }
        let _ = self.run(&["worktree", "prune"], None);
        Ok(())
    }

    fn exists_at(&self, reference: &str, path: &str) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["cat-file", "-e", &format!("{reference}:{path}")])
            .output()
            .context("running git cat-file for a control path")?;
        Ok(output.status.success())
    }

    pub(super) fn push_gitlab(&self, gitlab: &Gitlab, sha: &str, destination: &str) -> Result<()> {
        let url = format!(
            "{}/{}.git",
            self.config.gitlab_origin(),
            self.config.gitlab_project_path
        );
        self.run(
            &["push", &url, &format!("{sha}:refs/heads/{destination}")],
            Some(gitlab.git_header()),
        )?;
        Ok(())
    }

    pub(super) fn push_github(&self, github: &Github, sha: &str) -> Result<()> {
        let url = format!("https://github.com/{}.git", self.config.github_repo);
        self.run(
            &[
                "push",
                &url,
                &format!("{sha}:refs/heads/{}", self.config.branch),
            ],
            Some(github.git_header()),
        )?;
        Ok(())
    }

    fn run(&self, args: &[&str], header: Option<String>) -> Result<Vec<u8>> {
        let mut command = Command::new("git");
        command.current_dir(&self.root).args(args);
        if let Some(header) = header {
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env("GIT_CONFIG_VALUE_0", header);
        }
        let output = command
            .output()
            .with_context(|| format!("running git {}", args.first().unwrap_or(&"<unknown>")))?;
        checked(
            output,
            &format!("git {}", args.first().unwrap_or(&"<unknown>")),
        )
    }
}

fn checked(output: Output, label: &str) -> Result<Vec<u8>> {
    if !output.status.success() {
        bail!(
            "{label} failed with exit code {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

fn git_in(tree: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(tree)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.first().unwrap_or(&"<unknown>")))?;
    checked(
        output,
        &format!("git {}", args.first().unwrap_or(&"<unknown>")),
    )
}

#[cfg(test)]
mod tests {
    use reqwest::Url;
    use tempfile::TempDir;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("running git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn config(state: &Path) -> BridgeConfig {
        BridgeConfig {
            github_repo: "owner/repo".into(),
            github_token_file: state.join("github.token"),
            gitlab_url: Url::parse("https://gitlab.example.com").unwrap(),
            gitlab_project_id: 1,
            gitlab_project_path: "group/repo".into(),
            gitlab_username: "bot".into(),
            gitlab_token_file: state.join("gitlab.token"),
            branch: "main".into(),
            state_dir: state.to_path_buf(),
            pipeline_timeout_seconds: 60,
            pipeline_poll_seconds: 1,
        }
    }

    /// A base commit carrying a control path and a source file, then a head that
    /// changes both — the shape of a merged pull request that touches CI.
    fn repository() -> (TempDir, GitRepo, String, String) {
        let state = TempDir::new().unwrap();
        let repo = GitRepo::new(state.path(), &config(state.path())).unwrap();
        let work = state.path().join("work");
        fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        git(&work, &["config", "user.email", "t@e.st"]);
        git(&work, &["config", "user.name", "test"]);
        fs::create_dir_all(work.join("xtask/src")).unwrap();
        fs::write(work.join("xtask/src/main.rs"), "// trusted\n").unwrap();
        fs::write(work.join(".gitlab-ci.yml"), "trusted\n").unwrap();
        fs::write(work.join("src.rs"), "base\n").unwrap();
        git(&work, &["add", "--all"]);
        git(&work, &["commit", "--quiet", "-m", "base"]);
        let base = String::from_utf8(git_in(&work, &["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();

        fs::write(work.join("xtask/src/main.rs"), "// from the patch\n").unwrap();
        fs::write(work.join(".gitlab-ci.yml"), "from the patch\n").unwrap();
        fs::write(work.join("src.rs"), "head\n").unwrap();
        git(&work, &["add", "--all"]);
        git(&work, &["commit", "--quiet", "-m", "head"]);
        let head = String::from_utf8(git_in(&work, &["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();

        git(
            &work,
            &["push", "--quiet", repo.root.to_str().unwrap(), "main"],
        );
        (state, repo, base, head)
    }

    fn blob(repo: &GitRepo, reference: &str, path: &str) -> String {
        let bytes = repo
            .run(&["show", &format!("{reference}:{path}")], None)
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }

    /// The patch may not choose the pipeline that judges it.
    #[test]
    fn the_judged_commit_keeps_this_branch_ci() {
        let (_state, repo, base, head) = repository();

        let judged = repo.judged_commit(&base, &head).unwrap();

        assert_eq!(blob(&repo, &judged, ".gitlab-ci.yml"), "trusted\n");
        assert_eq!(blob(&repo, &judged, "xtask/src/main.rs"), "// trusted\n");
    }

    /// And it is still the patch that is being judged.
    #[test]
    fn the_judged_commit_carries_what_the_patch_changed() {
        let (_state, repo, base, head) = repository();

        let judged = repo.judged_commit(&base, &head).unwrap();

        assert_eq!(blob(&repo, &judged, "src.rs"), "head\n");
    }
}
