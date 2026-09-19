use std::fs;

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{
        parse::self_ty_name,
        violation::Violation,
        walker::{relative_to, workspace_rs_files_scoped},
    },
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivablePhase;

impl Check for DerivablePhase {
    fn id(&self) -> &'static str {
        "derivable_phase"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_phase;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let source = fs::read_to_string(&path)?;
            let relative = relative_to(ctx.workspace_root, &path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message = format!(
                    "structural typestate implementation for {name}: use #[derive(kithara_derive::Phase)]"
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
    let mut visitor = PhaseVisitor::default();
    visitor.visit_file(&file);
    visitor.findings
}

#[derive(Default)]
struct PhaseVisitor {
    findings: Vec<(String, usize)>,
}

impl<'ast> Visit<'ast> for PhaseVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let Some(trait_name) = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .map(|segment| segment.ident.to_string())
        else {
            syn::visit::visit_item_impl(self, implementation);
            return;
        };
        let supported = match trait_name.as_str() {
            "TrackPhase" => track_shape(implementation),
            "SegmentPhase" => {
                implementation.generics.where_clause.is_some() && data_only(implementation)
            }
            "ResourcePhase" => data_only(implementation),
            _ => false,
        };
        if supported && let Some(name) = self_ty_name(&implementation.self_ty) {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        syn::visit::visit_item_impl(self, implementation);
    }
}

fn data_only(implementation: &ItemImpl) -> bool {
    matches!(implementation.items.as_slice(), [ImplItem::Type(data)] if data.ident == "Data")
}

fn track_shape(implementation: &ItemImpl) -> bool {
    let [ImplItem::Type(data), ImplItem::Fn(erase)] = implementation.items.as_slice() else {
        return false;
    };
    if data.ident != "Data" || erase.sig.ident != "erase" {
        return false;
    }
    let [Stmt::Expr(Expr::Call(call), None)] = erase.block.stmts.as_slice() else {
        return false;
    };
    call.args.len() == 1
        && matches!(&*call.func, Expr::Path(path) if path.path.segments.len() == 2)
        && matches!(call.args.first(), Some(Expr::Path(path)) if path.path.is_ident("track"))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_all_supported_shapes() {
        let source = r#"
impl TrackPhase for Decoding { type Data = (); fn erase(track: Track<Self>) -> CurrentFsm { CurrentFsm::Decoding(track) } }
impl<S> SegmentPhase<S> for Loaded where S: HasPool<u8> { type Data = Proof; }
impl ResourcePhase for Reader { type Data<D: DriverIo> = ReadCore<D>; }
"#;
        assert_eq!(check_source(source).len(), 3);
    }

    #[test]
    fn ignores_behavior_and_missing_generic_bounds() {
        let source = r#"
impl Phase for Active { type Data = (); fn transition(self) {} }
impl<S> SegmentPhase<S> for Loaded { type Data = Proof; }
impl TrackPhase for Active { type Data = (); fn erase(track: Track<Self>) -> CurrentFsm { map(track) } }
"#;
        assert!(check_source(source).is_empty());
    }
}
