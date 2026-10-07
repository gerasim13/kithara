use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Lit, Stmt, visit, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableControl;

impl Check for DerivableControl {
    fn id(&self) -> &'static str {
        "derivable_control"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_control;
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
                    "structural Control implementation for {name}: use #[derive(kithara_derive::Control)]"
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
    let mut visitor = ControlVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct ControlVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for ControlVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_control = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Control");
        if is_control
            && implementation.attrs.is_empty()
            && structural_control(implementation)
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        visit::visit_item_impl(self, implementation);
    }
}

fn structural_control(implementation: &ItemImpl) -> bool {
    let mut saw_size = false;
    for item in &implementation.items {
        let ImplItem::Fn(method) = item else {
            return false;
        };
        let [Stmt::Expr(expr, None)] = method.block.stmts.as_slice() else {
            return false;
        };
        match method.sig.ident.to_string().as_str() {
            "size" if !saw_size && structural_size(expr) => saw_size = true,
            "composes_size" if matches!(expr, Expr::Lit(lit) if matches!(lit.lit, Lit::Bool(_))) => {
            }
            _ => return false,
        }
    }
    saw_size
}

fn structural_size(expr: &Expr) -> bool {
    match expr {
        Expr::Field(field) => structural_size(&field.base),
        Expr::Path(path) => !path.path.is_ident("self"),
        Expr::Call(call) => {
            matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "new" || segment.ident == "Fixed"))
                && call.args.iter().all(structural_size)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_field_fixed_and_composition_forms() {
        let source = r#"
impl Control for Cell { fn size(&self, skin: &SkinDoc) -> SizeSpec { skin.cell.size } }
impl Control for Divider { fn size(&self, skin: &SkinDoc) -> SizeSpec { SizeSpec::new(Dim::Fixed(skin.divider.width), Dim::Fill) } }
impl Control for Tab {
    fn composes_size(&self) -> bool { false }
    fn size(&self, skin: &SkinDoc) -> SizeSpec { skin.tab.size }
}
"#;
        assert_eq!(check_source(source).len(), 3);
    }

    #[test]
    fn ignores_behavior_extra_methods_and_cfg_specific_impls() {
        let source = r#"
impl Control for Dynamic { fn size(&self, skin: &SkinDoc) -> SizeSpec { self.pick(skin) } }
impl Control for Extra {
    fn size(&self, skin: &SkinDoc) -> SizeSpec { skin.cell.size }
    fn other(&self) {}
}
#[cfg(feature = "render")]
impl Control for BackendOnly { fn size(&self, skin: &SkinDoc) -> SizeSpec { skin.cell.size } }
"#;
        assert!(check_source(source).is_empty());
    }
}
