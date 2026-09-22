use std::fs;

use anyhow::Result;
use syn::{Expr, ExprMethodCall, ImplItem, ItemImpl, Stmt, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{parse::self_ty_name, violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableControlPainter;

impl Check for DerivableControlPainter {
    fn id(&self) -> &'static str {
        "derivable_control_painter"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_control_painter;
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
                    "structural ControlPainter implementation for {name}: use #[derive(kithara_derive::ControlPainter)]"
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
    let mut visitor = PainterVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct PainterVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for PainterVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_painter = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "ControlPainter");
        if is_painter
            && implementation.attrs.is_empty()
            && structural_painter(implementation)
            && let Some(name) = self_ty_name(&implementation.self_ty)
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        syn::visit::visit_item_impl(self, implementation);
    }
}

fn structural_painter(implementation: &ItemImpl) -> bool {
    let mut data = false;
    let mut draw = false;
    for item in &implementation.items {
        match item {
            ImplItem::Type(item) if item.ident == "Data" && !data => data = true,
            ImplItem::Const(item) if item.ident == "READS_POINTER" => {}
            ImplItem::Fn(method) if method.sig.ident == "draw" && !draw => {
                let [Stmt::Expr(expr, _)] = method.block.stmts.as_slice() else {
                    return false;
                };
                draw = matches!(
                    expr,
                    Expr::MethodCall(ExprMethodCall { receiver, .. })
                        if matches!(&**receiver, Expr::Path(path) if path.path.is_ident("self"))
                );
            }
            _ => return false,
        }
    }
    data && draw
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_direct_paint_with_optional_pointer_policy() {
        let source = r#"
impl ControlPainter for Chip {
    type Data = Labelled;
    fn draw(&self, list: &mut List, text: &mut Text, data: &Self::Data, bounds: Rect, _state: State) {
        self.paint(list, text, &data.label, bounds);
    }
}
impl ControlPainter for Button {
    type Data = bool;
    const READS_POINTER: bool = true;
    fn draw(&self, list: &mut List, _text: &mut Text, data: &Self::Data, bounds: Rect, state: State) {
        self.paint(list, *data, bounds, state);
    }
}
"#;
        assert_eq!(check_source(source).len(), 2);
    }

    #[test]
    fn ignores_branches_extra_methods_and_cfg_specific_impls() {
        let source = r#"
impl ControlPainter for Dynamic {
    type Data = bool;
    fn draw(&self, list: &mut List, text: &mut Text, data: &Self::Data, bounds: Rect, state: State) {
        if *data { self.paint(list, bounds) }
    }
}
impl ControlPainter for Extra {
    type Data = bool;
    fn draw(&self, list: &mut List, text: &mut Text, data: &Self::Data, bounds: Rect, state: State) {
        self.paint(list, bounds);
    }
    fn measure(&self) {}
}
#[cfg(feature = "render")]
impl ControlPainter for BackendOnly {
    type Data = bool;
    fn draw(&self, list: &mut List, text: &mut Text, data: &Self::Data, bounds: Rect, state: State) {
        self.paint(list, bounds);
    }
}
"#;
        assert!(check_source(source).is_empty());
    }
}
