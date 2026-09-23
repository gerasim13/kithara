use std::fs;

use anyhow::Result;
use syn::{
    Expr, ExprCall, ExprField, GenericArgument, GenericParam, ImplItem, ItemImpl, Member,
    PathArguments, Stmt, Type, visit, visit::Visit,
};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableClone;

impl Check for DerivableClone {
    fn id(&self) -> &'static str {
        "derivable_clone"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_clone;
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
                    "structural Clone for {name}: use #[derive(Clone)] or derive_where instead"
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
    let mut visitor = CloneVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct CloneVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for CloneVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_clone = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Clone");
        if is_clone
            && unspecialized(implementation)
            && implementation.items.len() == 1
            && implementation.items.iter().any(|item| {
                matches!(item, ImplItem::Fn(function) if function.sig.ident == "clone" && structural(&function.block.stmts))
            })
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        visit::visit_item_impl(self, implementation);
    }
}

fn unspecialized(implementation: &ItemImpl) -> bool {
    let Type::Path(self_ty) = &*implementation.self_ty else {
        return false;
    };
    let generics = implementation
        .generics
        .params
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(ty) => Some(&ty.ident),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(segment) = self_ty.path.segments.last() else {
        return false;
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return true;
    };
    arguments.args.iter().all(|argument| {
        let GenericArgument::Type(Type::Path(ty)) = argument else {
            return true;
        };
        ty.path
            .get_ident()
            .is_none_or(|ident| generics.contains(&ident))
    })
}

fn structural(statements: &[Stmt]) -> bool {
    let [Stmt::Expr(expression, None)] = statements else {
        return false;
    };
    match expression {
        Expr::Struct(value) => value.fields.iter().all(|field| cloned_field(&field.expr)),
        Expr::Call(call) => self_constructor(call) && call.args.iter().all(cloned_field),
        _ => false,
    }
}

fn self_constructor(call: &ExprCall) -> bool {
    matches!(&*call.func, Expr::Path(path) if path.path.is_ident("Self"))
}

fn cloned_field(expression: &Expr) -> bool {
    self_field(expression)
        || matches!(expression, Expr::MethodCall(call) if call.method == "clone" && call.args.is_empty() && self_field(&call.receiver))
        || matches!(expression, Expr::Call(call) if call.args.len() == 1 && call.args.first().is_some_and(self_field))
}

fn self_field(expression: &Expr) -> bool {
    let Expr::Field(ExprField { base, member, .. }) = expression else {
        return false;
    };
    matches!(member, Member::Named(_) | Member::Unnamed(_))
        && matches!(&**base, Expr::Path(path) if path.path.is_ident("self"))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_fieldwise_clone() {
        let findings = check_source(
            "impl<T> Clone for Handle<T> { fn clone(&self) -> Self { Self { inner: self.inner.clone(), count: self.count } } }",
        );
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn ignores_clone_with_state_change() {
        let findings = check_source(
            "impl Clone for Sender { fn clone(&self) -> Self { self.clones.fetch_add(1, Ordering::Relaxed); Self { inner: self.inner.clone() } } }",
        );
        assert!(findings.is_empty());
    }

    #[test]
    fn ignores_clone_for_one_typestate() {
        let findings = check_source(
            "impl<D> Clone for Resource<Reader, D> { fn clone(&self) -> Self { Self { data: self.data.clone() } } }",
        );
        assert!(findings.is_empty());
    }
}
