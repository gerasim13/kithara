use std::{collections::BTreeSet, fs};

use anyhow::Result;
use syn::{Expr, Fields, ImplItem, Item, ItemImpl, Lit, Stmt, visit::Visit};

use super::{Check, Context};
use crate::{
    common::{
        parse::self_ty_name,
        violation::Violation,
        walker::{relative_to, workspace_rs_files_scoped},
    },
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableSerialize;

impl Check for DerivableSerialize {
    fn id(&self) -> &'static str {
        "derivable_serialize"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_serialize;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let source = fs::read_to_string(&path)?;
            let relative = relative_to(ctx.workspace_root, &path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message = format!("unit-struct Serialize for {name}: use #[derive(Serialize)]");
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
    let unit_structs = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) if matches!(item.fields, Fields::Unit) => {
                Some(item.ident.to_string())
            }
            _ => None,
        })
        .collect();
    let mut visitor = SerializeVisitor {
        findings: Vec::new(),
        unit_structs,
    };
    visitor.visit_file(&file);
    visitor.findings
}

struct SerializeVisitor {
    findings: Vec<(String, usize)>,
    unit_structs: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for SerializeVisitor {
    fn visit_item_impl(&mut self, implementation: &'ast ItemImpl) {
        let is_serialize = implementation
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Serialize");
        if let Some(name) = self_ty_name(&implementation.self_ty)
            && is_serialize
            && self.unit_structs.contains(&name)
            && implementation.items.len() == 1
            && implementation.items.iter().any(|item| {
                matches!(item, ImplItem::Fn(function) if function.sig.ident == "serialize" && unit_serialization(&function.block.stmts, &name))
            })
        {
            self.findings
                .push((name, implementation.impl_token.span.start().line));
        }
        syn::visit::visit_item_impl(self, implementation);
    }
}

fn unit_serialization(statements: &[Stmt], type_name: &str) -> bool {
    let [Stmt::Expr(Expr::MethodCall(call), None)] = statements else {
        return false;
    };
    call.method == "serialize_unit_struct"
        && call.args.len() == 1
        && matches!(call.args.first(), Some(Expr::Lit(lit)) if matches!(&lit.lit, Lit::Str(name) if name.value() == type_name))
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_matching_unit_struct_wire() {
        let source = "struct Empty; impl Serialize for Empty { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { serializer.serialize_unit_struct(\"Empty\") } }";
        assert_eq!(check_source(source).len(), 1);
    }

    #[test]
    fn ignores_custom_wire_and_non_unit_structs() {
        let source = "struct Empty; impl Serialize for Empty { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { serializer.serialize_unit_struct(\"Alias\") } } struct Payload(u8); impl Serialize for Payload { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { serializer.serialize_u8(self.0) } }";
        assert!(check_source(source).is_empty());
    }
}
