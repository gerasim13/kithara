use anyhow::{Context as _, Result};
use quote::ToTokens;
use serde::Serialize;
use syn::{
    Attribute, FnArg, ImplItemFn, ImplItemType, ItemEnum, ItemFn, ItemImpl, ItemMod, ItemStruct,
    ItemType, Signature,
    visit::{self, Visit},
};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Declaration {
    source: String,
    scope: Vec<String>,
    name: String,
    line: usize,
    kind: &'static str,
    conditions: Vec<String>,
    attributes: Vec<String>,
    members: Vec<String>,
}

pub(super) fn discover(path: &str, source: &str) -> Result<Vec<Declaration>> {
    let syntax =
        syn::parse_file(source).with_context(|| format!("parse config discovery input {path}"))?;
    let mut visitor = Discovery {
        source: path,
        scope: Vec::new(),
        conditions: Vec::new(),
        declarations: Vec::new(),
    };
    visitor.visit_file(&syntax);
    Ok(visitor.declarations)
}

struct Discovery<'a> {
    source: &'a str,
    scope: Vec<String>,
    conditions: Vec<String>,
    declarations: Vec<Declaration>,
}

fn tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn conditions(attrs: &[Attribute]) -> impl Iterator<Item = String> + '_ {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
        .map(tokens)
}

fn is_candidate(name: &str, attrs: &[Attribute]) -> bool {
    ["Config", "Settings", "Options", "Params", "Policy", "Setup"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || attrs.iter().any(|attr| {
            attr.path().is_ident("config")
                || attr
                    .path()
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "config")
                || (attr.path().is_ident("derive")
                    && tokens(attr)
                        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                        .any(|word| word == "Builder"))
        })
}

impl Discovery<'_> {
    fn record(
        &mut self,
        name: &syn::Ident,
        kind: &'static str,
        attrs: &[Attribute],
        members: Vec<String>,
    ) {
        self.declarations.push(Declaration {
            source: self.source.to_owned(),
            scope: self.scope.clone(),
            name: name.to_string(),
            line: name.span().start().line,
            kind,
            conditions: self
                .conditions
                .iter()
                .cloned()
                .chain(conditions(attrs))
                .collect(),
            attributes: attrs.iter().map(tokens).collect(),
            members,
        });
    }

    fn constructor(&mut self, signature: &Signature, attrs: &[Attribute]) {
        if attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|part| part.ident == "builder")
        }) {
            self.record(
                &signature.ident,
                "builder_inputs",
                attrs,
                signature
                    .inputs
                    .iter()
                    .filter(|arg| matches!(arg, FnArg::Typed(_)))
                    .map(tokens)
                    .collect(),
            );
        }
    }
}

impl<'ast> Visit<'ast> for Discovery<'_> {
    fn visit_file(&mut self, file: &'ast syn::File) {
        let count = self.conditions.len();
        self.conditions.extend(conditions(&file.attrs));
        visit::visit_file(self, file);
        self.conditions.truncate(count);
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        if is_candidate(&item.ident.to_string(), &item.attrs) {
            self.record(
                &item.ident,
                "struct",
                &item.attrs,
                item.fields.iter().map(tokens).collect(),
            );
        }
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        if is_candidate(&item.ident.to_string(), &item.attrs) {
            self.record(
                &item.ident,
                "enum",
                &item.attrs,
                item.variants.iter().map(tokens).collect(),
            );
        }
        visit::visit_item_enum(self, item);
    }

    fn visit_item_type(&mut self, item: &'ast ItemType) {
        if is_candidate(&item.ident.to_string(), &item.attrs) {
            self.record(&item.ident, "alias", &item.attrs, vec![tokens(&item.ty)]);
        }
        visit::visit_item_type(self, item);
    }

    fn visit_impl_item_type(&mut self, item: &'ast ImplItemType) {
        if is_candidate(&item.ident.to_string(), &item.attrs) {
            self.record(
                &item.ident,
                "associated_alias",
                &item.attrs,
                vec![tokens(&item.ty)],
            );
        }
        visit::visit_impl_item_type(self, item);
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        self.scope.push(item.ident.to_string());
        visit::visit_item_mod(self, item);
        self.scope.pop();
        self.conditions.truncate(count);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        self.scope.push(match &item.trait_ {
            Some((path, _)) => format!("impl {} for {}", tokens(path), tokens(&item.self_ty)),
            None => format!("impl {}", tokens(&item.self_ty)),
        });
        visit::visit_item_impl(self, item);
        self.scope.pop();
        self.conditions.truncate(count);
    }

    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        self.constructor(&item.sig, &item.attrs);
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        self.scope.push(item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.scope.pop();
        self.conditions.truncate(count);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        self.constructor(&item.sig, &item.attrs);
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        self.scope.push(item.sig.ident.to_string());
        visit::visit_impl_item_fn(self, item);
        self.scope.pop();
        self.conditions.truncate(count);
    }
}

#[cfg(test)]
mod tests;
