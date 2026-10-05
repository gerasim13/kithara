use anyhow::Result;

use super::{Check, Context};
use crate::{
    common::{
        project::ProjectConfig,
        violation::Violation,
        walker::{relative_to, workspace_rs_files_scoped},
    },
    test::ignored::{declarations, reason_is_owned},
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "arch.ignored-test-owner";
}

pub(crate) struct IgnoredTestOwner;

impl Check for IgnoredTestOwner {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let project = ProjectConfig::load(ctx.workspace_root)?;
        let scope = ctx.scope.clone().with_workspace_sources();
        let mut violations = Vec::new();
        for path in workspace_rs_files_scoped(ctx.workspace_root, &scope)? {
            let Some(file) = ctx.parsed_file(&path)? else {
                continue;
            };
            let relative = relative_to(ctx.workspace_root, &path).display().to_string();
            for (name, line, reasons) in declarations(file) {
                let mut owned = true;
                for reason in reasons {
                    owned &= match reason {
                        Some(reason) => reason_is_owned(&reason, ctx.workspace_root, &project)?,
                        None => false,
                    };
                }
                if !owned {
                    violations.push(Violation::deny(
                        consts::ID,
                        format!("{relative}:{line}:{name}"),
                        "ignored test must name a configured lane, declared `run: just ...` recipe, or `issue: https://github.com/.../issues/N`; nightly kind must be red or flake",
                    ));
                }
            }
        }
        Ok(violations)
    }
}
