use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt, Type};

use super::{Check, Context};
use crate::{
    common::{
        parse::{collect_scopes, self_ty_name},
        violation::Violation,
        walker::relative_to,
    },
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableIntoProbeArg;

impl Check for DerivableIntoProbeArg {
    fn id(&self) -> &'static str {
        "derivable_into_probe_arg"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_into_probe_arg;
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
                    "one-way IntoProbeArg adapter for {name}: use #[derive(IntoProbeArg)] with #[probe_arg(encode_only)]"
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
    collect_scopes(&file)
        .into_iter()
        .flat_map(|scope| scope.impls.into_iter().filter_map(candidate))
        .collect()
}

fn candidate(implementation: &ItemImpl) -> Option<(String, usize)> {
    let (trait_path, _) = implementation.trait_.as_ref()?;
    if trait_path.segments.last()?.ident != "IntoProbeArg" {
        return None;
    }
    let name = target_name(&implementation.self_ty)?;
    if canonical_owner(&implementation.self_ty, &name) {
        return None;
    }
    let [ImplItem::Fn(method)] = implementation.items.as_slice() else {
        return None;
    };
    if method.sig.ident != "into_probe_arg" {
        return None;
    }
    let [statement] = method.block.stmts.as_slice() else {
        return None;
    };
    let Stmt::Expr(expression, _) = statement else {
        return None;
    };
    simple_projection(expression).then(|| (name, implementation.impl_token.span.start().line))
}

fn canonical_owner(ty: &Type, name: &str) -> bool {
    matches!(
        name,
        "bool" | "u64" | "i64" | "u32" | "i32" | "usize" | "Duration" | "Option" | "Url"
    ) || matches!(ty, Type::Reference(reference) if self_ty_name(&reference.elem).as_deref() == Some("Url"))
}

fn target_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Reference(reference) => self_ty_name(&reference.elem),
        _ => self_ty_name(ty),
    }
}

fn simple_projection(expression: &Expr) -> bool {
    match expression {
        Expr::Cast(cast) => rooted_in_self(&cast.expr),
        Expr::Call(call) => call.args.iter().any(rooted_in_self),
        Expr::MethodCall(call) => rooted_in_self(&call.receiver),
        _ => false,
    }
}

fn rooted_in_self(expression: &Expr) -> bool {
    match expression {
        Expr::Path(path) => path.path.is_ident("self"),
        Expr::Field(field) => rooted_in_self(&field.base),
        Expr::MethodCall(call) => rooted_in_self(&call.receiver),
        Expr::Call(call) => call.args.iter().any(rooted_in_self),
        Expr::Paren(paren) => rooted_in_self(&paren.expr),
        Expr::Reference(reference) => rooted_in_self(&reference.expr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn finds_newtype_unit_enum_and_projection_adapters() {
        let source = r#"
            impl IntoProbeArg for RequestId {
                fn into_probe_arg(self) -> u64 { self.0.get() }
            }
            impl IntoProbeArg for Priority {
                fn into_probe_arg(self) -> u64 { self as u64 }
            }
            impl IntoProbeArg for &Decision {
                fn into_probe_arg(self) -> u64 { self.target().get().to_u64().unwrap_or(0) }
            }
        "#;

        assert_eq!(
            check_source(source)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["RequestId", "Priority", "Decision"]
        );
    }

    #[test]
    fn ignores_canonical_and_non_projection_implementations() {
        let source = r#"
            impl IntoProbeArg for u64 {
                fn into_probe_arg(self) -> u64 { self }
                fn from_probe_arg(value: u64) -> Self { value }
            }
            impl IntoProbeArg for &Url {
                fn into_probe_arg(self) -> u64 { self.as_str().len() as u64 }
            }
            impl IntoProbeArg for Packed {
                fn into_probe_arg(self) -> u64 {
                    let high = self.high as u64;
                    high | self.low as u64
                }
            }
        "#;

        assert!(check_source(source).is_empty());
    }

    #[test]
    fn finds_an_adapter_in_a_test_module() {
        let source = "#[cfg(test)] mod tests { impl IntoProbeArg for RequestId { fn into_probe_arg(self) -> u64 { self.0.get() } } }";
        assert_eq!(check_source(source)[0].0, "RequestId");
    }
}
