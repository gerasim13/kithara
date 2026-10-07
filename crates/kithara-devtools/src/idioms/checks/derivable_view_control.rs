use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt, visit, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableViewControl;

impl Check for DerivableViewControl {
    fn id(&self) -> &'static str {
        "derivable_view_control"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_view_control;
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
                    "structural ViewControl implementation for {name}: use #[derive(kithara_derive::ViewControl)]"
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
    let mut visitor = ViewControlVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct ViewControlVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for ViewControlVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_view_control = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "ViewControl");
        if is_view_control
            && implementation.attrs.is_empty()
            && painted_view(implementation)
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        visit::visit_item_impl(self, implementation);
    }
}

fn painted_view(implementation: &ItemImpl) -> bool {
    let [ImplItem::Fn(view)] = implementation.items.as_slice() else {
        return false;
    };
    if view.sig.ident != "view" {
        return false;
    }
    let [Stmt::Expr(Expr::Call(call), None)] = view.block.stmts.as_slice() else {
        return false;
    };
    matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "painted"))
        && call.args.len() == 2
        && matches!(call.args.first(), Some(Expr::Path(path)) if path.path.is_ident("self"))
        && matches!(call.args.last(), Some(Expr::Path(path)) if path.path.is_ident("cx"))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_direct_painted_view() {
        let source = r#"
impl ViewControl for mount::Button {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        painted(self, cx)
    }
}
"#;
        assert_eq!(check_source(source), [("Button".to_owned(), 2)]);
    }

    #[test]
    fn ignores_behavior_extra_methods_and_cfg_specific_impls() {
        let source = r#"
impl ViewControl for Custom {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> { custom(self, cx) }
}
impl ViewControl for Extra {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> { painted(self, cx) }
    fn other(&self) {}
}
#[cfg(feature = "render")]
impl ViewControl for BackendOnly {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> { painted(self, cx) }
}
"#;
        assert!(check_source(source).is_empty());
    }
}
