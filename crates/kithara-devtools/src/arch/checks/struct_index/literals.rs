use std::collections::{BTreeMap, BTreeSet};

use quote::ToTokens;
use syn::{
    Expr, ExprCall, ExprMethodCall, ExprStruct, GenericParam, Generics, Item, Member, Signature,
    punctuated::Punctuated, token::Comma, visit, visit::Visit,
};

use super::index::{LiteralSite, SourceScope, WorkspaceStructIndex};
use crate::{arch::checks::declaration_index::DeclarationKey, common::imports::use_tree};

pub(super) struct LiteralVisitor<'a> {
    scope: SourceScope<'a>,
    pub(super) idx: &'a mut WorkspaceStructIndex,
    owner: Option<&'a DeclarationKey>,
    bindings: Bindings,
}

impl<'a> LiteralVisitor<'a> {
    pub(super) fn new(
        scope: SourceScope<'a>,
        idx: &'a mut WorkspaceStructIndex,
        owner: Option<&'a DeclarationKey>,
    ) -> Self {
        Self {
            scope,
            idx,
            owner,
            bindings: Bindings::default(),
        }
    }

    pub(super) fn bindings(
        &mut self,
        signature: &Signature,
        inherited: Option<&Generics>,
        block: &syn::Block,
    ) {
        self.bindings.visit_signature(signature);
        if let Some(inherited) = inherited {
            self.bindings.visit_generics(inherited);
        }
        self.bindings.visit_block(block);
    }

    fn is_shadowed(&self, path: &syn::Path) -> bool {
        let Some(first) = path.segments.first() else {
            return true;
        };
        let first = first.ident.to_string();
        path.leading_colon.is_none()
            && !matches!(first.as_str(), "crate" | "self" | "super" | "Self")
            && (self.bindings.unknown || self.bindings.names.contains(&first))
    }

    pub(super) fn resolve_path(&self, path: &syn::Path) -> Option<DeclarationKey> {
        if path.is_ident("Self") {
            return self.owner.cloned();
        }
        if self.is_shadowed(path) {
            return None;
        }
        self.scope
            .declarations
            .resolve_path(self.scope.rel, self.scope.inline, path)
    }

    fn record_arg(&mut self, expr: &ExprStruct, parent_fn: Option<DeclarationKey>) {
        if expr.qself.is_some() {
            return;
        }
        let Some(key) = self.resolve_path(&expr.path) else {
            return;
        };
        let mut field_exprs = BTreeMap::new();
        for field in &expr.fields {
            if let Member::Named(name) = &field.member {
                field_exprs.insert(name.to_string(), render_expr(&field.expr));
            }
        }
        self.idx.literals.entry(key).or_default().push(LiteralSite {
            field_exprs,
            parent_fn,
            has_rest: expr.rest.is_some(),
        });
    }

    fn visit_call_args(
        &mut self,
        args: &Punctuated<Expr, Comma>,
        parent_fn: Option<&DeclarationKey>,
    ) {
        for arg in args {
            if let Some(expr) = unwrap_argument_struct(arg) {
                self.record_arg(expr, parent_fn.cloned());
                for field in &expr.fields {
                    self.visit_expr(&field.expr);
                }
                if let Some(rest) = &expr.rest {
                    self.visit_expr(rest);
                }
            } else {
                self.visit_expr(arg);
            }
        }
    }
}

impl<'ast> Visit<'ast> for LiteralVisitor<'_> {
    fn visit_expr_call(&mut self, expr: &'ast ExprCall) {
        let parent = match expr.func.as_ref() {
            Expr::Path(path) if path.qself.is_none() && !self.is_shadowed(&path.path) => self
                .scope
                .declarations
                .resolve_function(self.scope.rel, self.scope.inline, &path.path),
            _ => None,
        };
        self.visit_expr(&expr.func);
        self.visit_call_args(&expr.args, parent.as_ref());
    }

    fn visit_expr_method_call(&mut self, expr: &'ast ExprMethodCall) {
        self.visit_expr(&expr.receiver);
        self.visit_call_args(&expr.args, None);
    }

    fn visit_expr_struct(&mut self, expr: &'ast ExprStruct) {
        self.record_arg(expr, None);
        for field in &expr.fields {
            self.visit_expr(&field.expr);
        }
        if let Some(rest) = &expr.rest {
            self.visit_expr(rest);
        }
    }

    fn visit_item(&mut self, _item: &'ast Item) {}
}

#[derive(Default)]
struct Bindings {
    names: BTreeSet<String>,
    unknown: bool,
}

impl<'ast> Visit<'ast> for Bindings {
    fn visit_generic_param(&mut self, parameter: &'ast GenericParam) {
        match parameter {
            GenericParam::Type(parameter) => {
                self.names.insert(parameter.ident.to_string());
            }
            GenericParam::Const(parameter) => {
                self.names.insert(parameter.ident.to_string());
            }
            GenericParam::Lifetime(_) => {}
        }
    }

    fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
        self.names.insert(pattern.ident.to_string());
        visit::visit_pat_ident(self, pattern);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        let name = match item {
            Item::Struct(item) => Some(&item.ident),
            Item::Enum(item) => Some(&item.ident),
            Item::Type(item) => Some(&item.ident),
            Item::Mod(item) => Some(&item.ident),
            Item::Fn(item) => Some(&item.sig.ident),
            Item::Use(item) => {
                let mut imports = Vec::new();
                use_tree(&item.tree, &mut Vec::new(), &mut imports);
                for import in imports {
                    if import.alias == "*" {
                        self.unknown = true;
                    } else {
                        self.names.insert(import.alias);
                    }
                }
                None
            }
            _ => {
                self.unknown = true;
                None
            }
        };
        if let Some(name) = name {
            self.names.insert(name.to_string());
        }
    }

    fn visit_stmt_macro(&mut self, _item: &'ast syn::StmtMacro) {
        self.unknown = true;
    }
}

fn unwrap_argument_struct(expr: &Expr) -> Option<&ExprStruct> {
    match expr {
        Expr::Struct(expr) => Some(expr),
        Expr::Reference(expr) => unwrap_argument_struct(&expr.expr),
        Expr::Paren(expr) => unwrap_argument_struct(&expr.expr),
        _ => None,
    }
}

fn render_expr(expr: &Expr) -> String {
    expr.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn full_literal_sites(sites: &[LiteralSite]) -> Vec<&LiteralSite> {
    sites.iter().filter(|site| !site.has_rest).collect()
}

pub(crate) fn unique_consumer(sites: &[LiteralSite]) -> Option<&DeclarationKey> {
    let mut single = None;
    for site in sites {
        let parent = site.parent_fn.as_ref()?;
        match single {
            None => single = Some(parent),
            Some(previous) if previous == parent => {}
            Some(_) => return None,
        }
    }
    single
}
