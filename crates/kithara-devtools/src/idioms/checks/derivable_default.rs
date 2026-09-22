use std::fs;

use anyhow::Result;
use syn::{Expr, ExprCall, ImplItem, ItemImpl, Stmt, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableDefault;

impl Check for DerivableDefault {
    fn id(&self) -> &'static str {
        "derivable_default"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_default;
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
                    "structural Default for {name}: use #[derive(Default)] or derive_where"
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
    let mut visitor = DefaultVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct DefaultVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for DefaultVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_default = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Default");
        if is_default
            && implementation.items.len() == 1
            && implementation.items.iter().any(|item| {
                matches!(item, ImplItem::Fn(function) if function.sig.ident == "default" && structural(&function.block.stmts))
            })
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        syn::visit::visit_item_impl(self, implementation);
    }
}

fn structural(statements: &[Stmt]) -> bool {
    let [Stmt::Expr(expression, None)] = statements else {
        return false;
    };
    match expression {
        Expr::Struct(value) => {
            value.rest.is_none() && value.fields.iter().all(|field| neutral(&field.expr))
        }
        Expr::Call(call) => self_constructor(call) && call.args.iter().all(neutral),
        _ => false,
    }
}

fn self_constructor(call: &ExprCall) -> bool {
    matches!(&*call.func, Expr::Path(path) if path.path.is_ident("Self"))
}

fn neutral(expression: &Expr) -> bool {
    match expression {
        Expr::Call(call) => {
            let Expr::Path(function) = &*call.func else {
                return false;
            };
            let Some(method) = function.path.segments.last() else {
                return false;
            };
            if method.ident == "default" {
                return call.args.is_empty();
            }
            method.ident == "new"
                && function
                    .path
                    .segments
                    .iter()
                    .rev()
                    .nth(1)
                    .is_some_and(|owner| {
                        matches!(
                            owner.ident.to_string().as_str(),
                            "BTreeMap"
                                | "BTreeSet"
                                | "HashMap"
                                | "HashSet"
                                | "Mutex"
                                | "RwLock"
                                | "Vec"
                                | "VecDeque"
                        )
                    })
                && call.args.iter().all(neutral)
        }
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "None" || segment.ident == "PhantomData"),
        Expr::Tuple(tuple) => tuple.elems.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_structural_defaults() {
        let source = "impl<T> Default for Cache<T> { fn default() -> Self { Self { values: Vec::new(), marker: PhantomData } } } impl<T: Default> Default for Gate<T> { fn default() -> Self { Self(Mutex::new(T::default())) } }";
        assert_eq!(check_source(source).len(), 2);
    }

    #[test]
    fn ignores_defaults_with_policy() {
        let source = "impl Default for Config { fn default() -> Self { Self { capacity: DEFAULT_CAPACITY } } } impl Default for Clock { fn default() -> Self { Self::new(Instant::now()) } }";
        assert!(check_source(source).is_empty());
    }
}
