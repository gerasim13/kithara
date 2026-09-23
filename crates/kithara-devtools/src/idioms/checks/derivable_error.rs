use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt, visit, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableError;

impl Check for DerivableError {
    fn id(&self) -> &'static str {
        "derivable_error"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_error;
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
                    "mechanical Error for {name}: use derive_more::Error with explicit source fields"
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
    let Ok(file) = syn::parse_file(source) else {
        return Vec::new();
    };
    let mut visitor = ErrorVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct ErrorVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for ErrorVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_error = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Error");
        if is_error
            && (implementation.items.is_empty()
                || implementation.items.iter().all(|item| {
                    matches!(item, ImplItem::Fn(function) if function.sig.ident == "source" && mechanical_source(&function.block.stmts))
                }))
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        visit::visit_item_impl(self, implementation);
    }
}

fn mechanical_source(statements: &[Stmt]) -> bool {
    let [Stmt::Expr(expression, None)] = statements else {
        return false;
    };
    source_expression(expression)
}

fn source_expression(expression: &Expr) -> bool {
    match expression {
        Expr::Match(value) => value.arms.iter().all(|arm| source_expression(&arm.body)),
        Expr::Call(call) => {
            matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Some"))
                && call.args.len() == 1
                && matches!(call.args.first(), Some(Expr::Path(_)))
        }
        Expr::Path(path) => path.path.is_ident("None"),
        Expr::Paren(paren) => source_expression(&paren.expr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_empty_error() {
        assert_eq!(check_source("impl Error for Closed {} ").len(), 1);
    }

    #[test]
    fn reports_structural_source_chain() {
        let source = "impl Error for LoadError { fn source(&self) -> Option<&(dyn Error + 'static)> { match self { Self::Read { source } => Some(source), Self::Missing => None } } }";
        assert_eq!(check_source(source).len(), 1);
    }

    #[test]
    fn ignores_computed_source() {
        let source = "impl Error for Routed { fn source(&self) -> Option<&(dyn Error + 'static)> { self.lookup_source() } }";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn ignores_source_that_unwraps_an_arc() {
        let source = "impl Error for CleanupError { fn source(&self) -> Option<&(dyn Error + 'static)> { Some(self.source.as_ref()) } }";
        assert!(check_source(source).is_empty());
    }
}
