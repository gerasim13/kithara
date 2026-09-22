use anyhow::{Context as _, Result};
use quote::ToTokens;
use serde::Serialize;
use syn::{
    Attribute, Field, FnArg, ImplItemFn, ImplItemType, ItemEnum, ItemFn, ItemImpl, ItemMod,
    ItemStruct, ItemType, LitStr, Signature,
    visit::{self, Visit},
};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Registration {
    pub(super) source: String,
    pub(super) package: String,
    pub(super) module_path: String,
    scope: Vec<String>,
    pub(super) owner: String,
    pub(super) property: Option<String>,
    pub(super) hook: Option<String>,
    pub(super) kind: &'static str,
    pub(super) sdk: bool,
    pub(super) docs: Vec<String>,
    conditions: Vec<String>,
    pub(super) fields: Vec<RegisteredField>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct RegisteredField {
    pub(super) name: String,
    pub(super) rust_type: String,
    pub(super) role: String,
    update: bool,
    exclusion_reason: Option<String>,
    pub(super) docs: Vec<String>,
}

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

pub(super) fn registrations(path: &str, source: &str) -> Result<Vec<Registration>> {
    let syntax = syn::parse_file(source)
        .with_context(|| format!("parse config registration input {path}"))?;
    let mut visitor = Registrations {
        source: path,
        package: source_identity(path).0,
        module_path: source_identity(path).1,
        scope: Vec::new(),
        conditions: Vec::new(),
        owner: None,
        registrations: Vec::new(),
        error: None,
    };
    visitor.visit_file(&syntax);
    if let Some(error) = visitor.error {
        return Err(error).with_context(|| format!("parse config registration input {path}"));
    }
    Ok(visitor.registrations)
}

struct Registrations<'a> {
    source: &'a str,
    package: String,
    module_path: String,
    scope: Vec<String>,
    conditions: Vec<String>,
    owner: Option<String>,
    registrations: Vec<Registration>,
    error: Option<anyhow::Error>,
}

fn source_identity(path: &str) -> (String, String) {
    let parts: Vec<_> = path.split('/').collect();
    let (package, source) = match parts.as_slice() {
        ["crates", package, "src", rest @ ..] | ["tests", "crates", package, "src", rest @ ..] => {
            ((*package).to_owned(), rest)
        }
        ["xtask", "src", rest @ ..] => ("xtask".to_owned(), rest),
        ["src", rest @ ..] => ("kithara".to_owned(), rest),
        _ => ("workspace".to_owned(), parts.as_slice()),
    };
    let mut modules: Vec<_> = source
        .iter()
        .map(|part| part.trim_end_matches(".rs"))
        .collect();
    if modules
        .last()
        .is_some_and(|name| matches!(*name, "lib" | "main" | "mod"))
    {
        modules.pop();
    }
    (package, modules.join("::"))
}

fn config_attribute(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "config")
    })
}

fn docs(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(value) => match &value.value {
                syn::Expr::Lit(expr) => match &expr.lit {
                    syn::Lit::Str(text) => Some(text.value().trim().to_owned()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn registered_field(field: &Field) -> syn::Result<RegisteredField> {
    let name = field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(field, "config requires named fields"))?;
    let attr = config_attribute(&field.attrs)
        .ok_or_else(|| syn::Error::new_spanned(field, "registered config field is unclassified"))?;
    let mut role = None;
    let mut update = false;
    let mut exclusion_reason = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("update") {
            update = true;
        } else if meta.path.is_ident("value") {
            role = Some("value");
            let _: proc_macro2::TokenStream = meta.input.parse()?;
        } else if meta.path.is_ident("nested") {
            role = Some("nested");
        } else if meta.path.is_ident("skip") {
            role = Some("skip");
            exclusion_reason = Some(meta.value()?.parse::<LitStr>()?.value());
        }
        Ok(())
    })?;
    Ok(RegisteredField {
        name: name.to_string(),
        rust_type: tokens(&field.ty),
        role: role.unwrap_or("unknown").to_owned(),
        update,
        exclusion_reason,
        docs: docs(&field.attrs),
    })
}

impl<'ast> Visit<'ast> for Registrations<'_> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        self.scope.push(item.ident.to_string());
        visit::visit_item_mod(self, item);
        self.scope.pop();
        self.conditions.truncate(count);
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        if let Some(attribute) = config_attribute(&item.attrs) {
            let mut sdk = false;
            if matches!(attribute.meta, syn::Meta::List(_))
                && let Err(error) = attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("sdk") {
                        sdk = true;
                    } else if meta.input.peek(syn::Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                })
            {
                self.error = Some(error.into());
                return;
            }
            match item.fields.iter().map(registered_field).collect() {
                Ok(fields) => self.registrations.push(Registration {
                    source: self.source.to_owned(),
                    package: self.package.clone(),
                    module_path: self.module_path.clone(),
                    scope: self.scope.clone(),
                    owner: item.ident.to_string(),
                    property: None,
                    hook: None,
                    kind: "retained",
                    sdk,
                    docs: docs(&item.attrs),
                    conditions: self
                        .conditions
                        .iter()
                        .cloned()
                        .chain(conditions(&item.attrs))
                        .collect(),
                    fields,
                }),
                Err(error) => self.error = Some(error.into()),
            }
        }
        visit::visit_item_struct(self, item);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        let count = self.conditions.len();
        self.conditions.extend(conditions(&item.attrs));
        let owner = self.owner.replace(tokens(&item.self_ty));
        self.scope.push(format!("impl {}", tokens(&item.self_ty)));
        visit::visit_item_impl(self, item);
        self.scope.pop();
        self.owner = owner;
        self.conditions.truncate(count);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        let Some(attr) = config_attribute(&item.attrs) else {
            return visit::visit_impl_item_fn(self, item);
        };
        let mut property = None;
        let mut sdk = false;
        if let Err(error) = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("delegate") {
                property = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("sdk") {
                sdk = true;
            }
            Ok(())
        }) {
            self.error = Some(error.into());
            return;
        }
        let fields = item
            .sig
            .inputs
            .iter()
            .filter_map(|arg| match arg {
                FnArg::Typed(input) => Some(RegisteredField {
                    name: match &*input.pat {
                        syn::Pat::Ident(name) => name.ident.to_string(),
                        pattern => tokens(pattern),
                    },
                    rust_type: tokens(&input.ty),
                    role: "delegate_input".to_owned(),
                    update: true,
                    exclusion_reason: None,
                    docs: Vec::new(),
                }),
                FnArg::Receiver(_) => None,
            })
            .collect();
        self.registrations.push(Registration {
            source: self.source.to_owned(),
            package: self.package.clone(),
            module_path: self.module_path.clone(),
            scope: self.scope.clone(),
            owner: self.owner.clone().unwrap_or_default(),
            property,
            hook: Some(item.sig.ident.to_string()),
            kind: "delegate",
            sdk,
            docs: docs(&item.attrs),
            conditions: self
                .conditions
                .iter()
                .cloned()
                .chain(conditions(&item.attrs))
                .collect(),
            fields,
        });
        visit::visit_impl_item_fn(self, item);
    }
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

fn config_name(name: &str) -> bool {
    [
        "Config",
        "Configuration",
        "Settings",
        "Options",
        "Params",
        "Policy",
        "Setup",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

struct ConfigInput(bool);

impl<'ast> Visit<'ast> for ConfigInput {
    fn visit_type_path(&mut self, ty: &'ast syn::TypePath) {
        self.0 |= ty
            .path
            .segments
            .last()
            .is_some_and(|segment| config_name(&segment.ident.to_string()));
        visit::visit_type_path(self, ty);
    }
}

impl Discovery<'_> {
    fn is_candidate(&self, name: &str, attrs: &[Attribute]) -> bool {
        self.source
            .split('/')
            .any(|part| matches!(part, "config" | "config.rs" | "schema" | "schema.rs"))
            || self
                .scope
                .iter()
                .any(|part| matches!(part.as_str(), "config" | "schema"))
            || config_name(name)
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
                            .any(|word| matches!(word, "Builder" | "Patch")))
            })
    }
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
        let builder = attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|part| part.ident == "builder")
        });
        let mut config_input = ConfigInput(false);
        for input in &signature.inputs {
            if let FnArg::Typed(input) = input {
                config_input.visit_type(&input.ty);
            }
        }
        if builder || config_input.0 {
            self.record(
                &signature.ident,
                if builder {
                    "builder_inputs"
                } else {
                    "config_inputs"
                },
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
        if self.is_candidate(&item.ident.to_string(), &item.attrs) {
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
        if self.is_candidate(&item.ident.to_string(), &item.attrs) {
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
        if self.is_candidate(&item.ident.to_string(), &item.attrs) {
            self.record(&item.ident, "alias", &item.attrs, vec![tokens(&item.ty)]);
        }
        visit::visit_item_type(self, item);
    }

    fn visit_impl_item_type(&mut self, item: &'ast ImplItemType) {
        if self.is_candidate(&item.ident.to_string(), &item.attrs) {
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
