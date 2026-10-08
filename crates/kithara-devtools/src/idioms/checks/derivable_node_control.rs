use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt};

use super::{Check, Context, derivable_support::check_impls};
use crate::{
    common::{violation::Violation, walker::relative_to},
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableNodeControl;

impl Check for DerivableNodeControl {
    fn id(&self) -> &'static str {
        "derivable_node_control"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_node_control;
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
                    "structural NodeControl implementation for {name}: use #[derive(kithara_derive::NodeControl)]"
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
    check_impls(source, "NodeControl", |implementation| {
        implementation.attrs.is_empty() && painted_leaf(implementation)
    })
}

fn painted_leaf(implementation: &ItemImpl) -> bool {
    let [ImplItem::Fn(leaf)] = implementation.items.as_slice() else {
        return false;
    };
    if leaf.sig.ident != "leaf" {
        return false;
    }
    let [Stmt::Expr(Expr::Call(call), None)] = leaf.block.stmts.as_slice() else {
        return false;
    };
    matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "painted"))
        && call.args.len() == 3
        && matches!(call.args.first(), Some(Expr::Path(path)) if path.path.is_ident("self"))
        && matches!(call.args.get(1), Some(Expr::Path(path)) if path.path.is_ident("host"))
        && matches!(call.args.last(), Some(Expr::Path(path)) if path.path.is_ident("cx"))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_direct_painted_leaf() {
        let source = r#"
impl NodeControl for mount::Button {
    fn leaf<A>(&self, host: &Host<'_, A>, cx: &Cx<'_>) -> Node<A>
    where A: Debug + Send + 'static {
        painted(self, host, cx)
    }
}
"#;
        assert_eq!(check_source(source), [("Button".to_owned(), 2)]);
    }

    #[test]
    fn ignores_runtime_checks_extra_methods_and_cfg_specific_impls() {
        let source = r#"
impl NodeControl for Hosted {
    fn leaf<A>(&self, host: &Host<'_, A>, cx: &Cx<'_>) -> Node<A> { hosted(self, host, cx) }
}
impl NodeControl for Extra {
    fn leaf<A>(&self, host: &Host<'_, A>, cx: &Cx<'_>) -> Node<A> { painted(self, host, cx) }
    fn wire(&self) {}
}
#[cfg(feature = "masonry")]
impl NodeControl for BackendOnly {
    fn leaf<A>(&self, host: &Host<'_, A>, cx: &Cx<'_>) -> Node<A> { painted(self, host, cx) }
}
"#;
        assert!(check_source(source).is_empty());
    }
}
