use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, Stmt};

use super::{Check, Context, derivable_support::check_impls};
use crate::{
    common::{violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableBuiltDefault;

impl Check for DerivableBuiltDefault {
    fn id(&self) -> &'static str {
        "derivable_built_default"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_built_default;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let source = fs::read_to_string(path)?;
            let relative = relative_to(ctx.workspace_root, path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message = format!(
                    "builder-backed Default for {name}: use #[derive(kithara_derive::BuiltDefault)]"
                );
                out.push(match config.severity {
                    DerivableSeverity::Deny => Violation::deny(self.id(), key, message),
                    DerivableSeverity::Warn => Violation::warn(self.id(), key, message),
                });
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}

fn check_source(source: &str) -> Vec<(String, usize)> {
    check_impls(source, "Default", |implementation| {
        implementation.items.len() == 1
            && implementation.items.iter().any(|item| {
                matches!(item, ImplItem::Fn(function) if function.sig.ident == "default" && builder_build(&function.block.stmts))
            })
    })
}

fn builder_build(statements: &[Stmt]) -> bool {
    let [Stmt::Expr(Expr::MethodCall(build), None)] = statements else {
        return false;
    };
    build.method == "build"
        && build.args.is_empty()
        && matches!(&*build.receiver, Expr::Call(builder)
            if builder.args.is_empty()
                && matches!(&*builder.func, Expr::Path(path)
                    if path.path.segments.len() == 2
                        && path.path.segments.first().is_some_and(|segment| segment.ident == "Self")
                        && path.path.segments.last().is_some_and(|segment| segment.ident == "builder")))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_plain_builder_default() {
        assert_eq!(
            check_source(
                "impl Default for Config { fn default() -> Self { Self::builder().build() } }"
            )
            .len(),
            1
        );
    }

    #[test]
    fn ignores_builder_with_policy_calls() {
        assert!(check_source("impl<B: Default> Default for Config<B> { fn default() -> Self { Self::builder().backend(B::default()).build() } }").is_empty());
    }
}
