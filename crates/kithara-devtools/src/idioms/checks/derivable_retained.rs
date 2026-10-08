use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt};

use super::{Check, Context, derivable_support::check_impls};
use crate::{
    common::{violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableRetained;

impl Check for DerivableRetained {
    fn id(&self) -> &'static str {
        "derivable_retained"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_retained;
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
                    "structural Retained implementation for {name}: use #[derive(kithara_derive::Retained)]"
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
    check_impls(source, "Retained", |implementation| {
        implementation.attrs.is_empty() && structural_retained(implementation)
    })
}

fn structural_retained(implementation: &ItemImpl) -> bool {
    if implementation.items.is_empty() {
        return true;
    }
    let [ImplItem::Fn(method)] = implementation.items.as_slice() else {
        return false;
    };
    let [Stmt::Expr(Expr::Call(call), _)] = method.block.stmts.as_slice() else {
        return false;
    };
    method.sig.ident == "set_read"
        && matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| matches!(segment.ident.to_string().as_str(), "set_bool" | "set_scalar" | "set_levels" | "set_labelled")))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_empty_and_direct_setter_forms() {
        let source = r#"
impl Retained for Brand {}
impl Retained for Button {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_bool(&mut data.active, value)
    }
}
"#;
        assert_eq!(check_source(source).len(), 2);
    }

    #[test]
    fn ignores_behavior_extra_methods_and_cfg_specific_impls() {
        let source = r#"
impl Retained for Dynamic {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool { data.update(value) }
}
impl Retained for Extra {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool { set_bool(data, value) }
    fn other() {}
}
#[cfg(feature = "masonry")]
impl Retained for BackendOnly {}
"#;
        assert!(check_source(source).is_empty());
    }
}
