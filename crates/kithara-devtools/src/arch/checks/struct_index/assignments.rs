use std::collections::HashSet;

use syn::{BinOp, Expr, ExprAssign, ExprBinary, Member, visit, visit::Visit};

use super::index::WorkspaceStructIndex;

pub(crate) fn crate_name_from_rel(rel: &str) -> Option<&str> {
    let mut components = rel.split('/');
    if components.next() == Some("crates") {
        components.next()
    } else {
        None
    }
}

struct AssignedFieldVisitor<'a> {
    fields: &'a mut HashSet<String>,
}

impl AssignedFieldVisitor<'_> {
    fn record_assignment(&mut self, expr: &Expr) {
        let Expr::Field(field) = expr else {
            return;
        };
        let Member::Named(field_name) = &field.member else {
            return;
        };
        self.fields.insert(field_name.to_string());
    }
}

impl<'ast> Visit<'ast> for AssignedFieldVisitor<'_> {
    fn visit_expr_assign(&mut self, n: &'ast ExprAssign) {
        self.record_assignment(&n.left);
        visit::visit_expr_assign(self, n);
    }

    fn visit_expr_binary(&mut self, n: &'ast ExprBinary) {
        if matches!(
            n.op,
            BinOp::AddAssign(_)
                | BinOp::SubAssign(_)
                | BinOp::MulAssign(_)
                | BinOp::DivAssign(_)
                | BinOp::RemAssign(_)
                | BinOp::BitXorAssign(_)
                | BinOp::BitAndAssign(_)
                | BinOp::BitOrAssign(_)
                | BinOp::ShlAssign(_)
                | BinOp::ShrAssign(_)
        ) {
            self.record_assignment(&n.left);
        }
        visit::visit_expr_binary(self, n);
    }
}

pub(super) fn collect_assigned_fields(file: &syn::File, rel: &str, idx: &mut WorkspaceStructIndex) {
    let Some(crate_name) = crate_name_from_rel(rel) else {
        return;
    };
    let fields = idx
        .assigned_fields
        .entry(crate_name.to_string())
        .or_default();
    AssignedFieldVisitor { fields }.visit_file(file);
}
