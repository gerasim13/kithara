use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, Lit, Stmt};

use super::{Check, Context, derivable_support::check_impls};
use crate::{
    common::{violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableDebug;

impl Check for DerivableDebug {
    fn id(&self) -> &'static str {
        "derivable_debug"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_debug;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let source = fs::read_to_string(path)?;
            let relative = relative_to(ctx.workspace_root, path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message =
                    format!("mechanical Debug for {name}: use std or derive_more::Debug instead");
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
    check_impls(source, "Debug", |implementation| {
        implementation.items.len() == 1
            && implementation.items.iter().any(|item| {
                matches!(item, ImplItem::Fn(function) if function.sig.ident == "fmt" && mechanical(&function.block.stmts))
            })
    })
}

fn mechanical(statements: &[Stmt]) -> bool {
    let [Stmt::Expr(expression, None)] = statements else {
        return false;
    };
    match expression {
        Expr::MethodCall(call) if call.args.len() == 1 => {
            (call.method == "fmt" && rooted_in_self(&call.receiver))
                || (call.method == "write_str"
                    && matches!(call.args.first(), Some(Expr::Lit(lit)) if matches!(lit.lit, Lit::Str(_))))
        }
        _ => false,
    }
}

fn rooted_in_self(expression: &Expr) -> bool {
    match expression {
        Expr::Field(field) => rooted_in_self(&field.base),
        Expr::MethodCall(call) => rooted_in_self(&call.receiver),
        Expr::Path(path) => path.path.is_ident("self"),
        Expr::Paren(paren) => rooted_in_self(&paren.expr),
        Expr::Reference(reference) => rooted_in_self(&reference.expr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_transparent_debug() {
        let findings = check_source(
            "impl<T: Debug> Debug for Wrapper<T> { fn fmt(&self, f: &mut Formatter<'_>) -> Result { self.inner.fmt(f) } }",
        );
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn ignores_algorithmic_debug() {
        let findings = check_source(
            "impl Debug for State { fn fmt(&self, f: &mut Formatter<'_>) -> Result { if self.ready() { f.write_str(\"ready\") } else { f.write_str(\"waiting\") } } }",
        );
        assert!(findings.is_empty());
    }
}
