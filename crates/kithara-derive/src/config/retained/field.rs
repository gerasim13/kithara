use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, Field, GenericParam, Generics, LitStr, Result, Type, parenthesized, visit::Visit as _,
};

enum Role {
    Value,
    Projection(Box<(Type, Expr)>),
    Nested,
    Skip,
}

pub(super) fn expand(
    field: &mut Field,
    generics: &Generics,
) -> Result<Option<(TokenStream, TokenStream)>> {
    let mut role = None;
    let mut preserved: Vec<syn::Attribute> = Vec::new();
    for attr in &field.attrs {
        if !attr.path().is_ident("config") {
            preserved.push(attr.clone());
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if role.is_some() {
                return Err(meta.error("select exactly one config field role"));
            }
            role = Some(if meta.path.is_ident("value") {
                if meta.input.peek(syn::token::Paren) {
                    let content;
                    parenthesized!(content in meta.input);
                    let ty = content.parse()?;
                    content.parse::<syn::Token![,]>()?;
                    let expression = content.parse()?;
                    if !content.is_empty() {
                        return Err(content.error("unexpected projection tokens"));
                    }
                    Role::Projection(Box::new((ty, expression)))
                } else {
                    Role::Value
                }
            } else if meta.path.is_ident("nested") {
                Role::Nested
            } else if meta.path.is_ident("skip") {
                let reason: LitStr = meta.value()?.parse()?;
                if reason.value().trim().is_empty() {
                    return Err(meta.error("config exclusion requires a reason"));
                }
                Role::Skip
            } else {
                return Err(
                    meta.error("expected value, value(Type, expression), nested, or skip = reason")
                );
            });
            Ok(())
        })?;
    }
    let role = role.ok_or_else(|| {
        syn::Error::new_spanned(
            &*field,
            "classify each config field as value, nested, or skip = reason",
        )
    })?;
    field.attrs = preserved;
    let name = field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(&*field, "config requires named fields"))?;
    let original_type = &field.ty;
    let (ty, expression): (Type, Expr) = match role {
        Role::Skip => return Ok(None),
        Role::Value => (
            original_type.clone(),
            syn::parse_quote!(::core::clone::Clone::clone(&self.#name)),
        ),
        Role::Nested => (
            syn::parse_quote!(<#original_type as ::kithara_config::Config>::Values),
            syn::parse_quote!(::kithara_config::Config::values(&self.#name)),
        ),
        Role::Projection(projection) => *projection,
    };
    let mut usage = GenericUse {
        generics,
        found: false,
    };
    usage.visit_type(&ty);
    if usage.found {
        return Err(syn::Error::new_spanned(
            ty,
            "snapshot types cannot depend on resource generics; use value(OwnedType, expression)",
        ));
    }
    let gates = super::attributes(&field.attrs, false)?;
    let surface = super::attributes(&field.attrs, true)?;
    Ok(Some((
        quote! { #(#surface)* pub #name: #ty },
        quote! { #(#gates)* #name: #expression },
    )))
}

struct GenericUse<'a> {
    generics: &'a Generics,
    found: bool,
}

impl<'ast> syn::visit::Visit<'ast> for GenericUse<'_> {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.found |= self
            .generics
            .params
            .iter()
            .any(|parameter| match parameter {
                GenericParam::Type(parameter) => parameter.ident == *ident,
                GenericParam::Const(parameter) => parameter.ident == *ident,
                GenericParam::Lifetime(parameter) => parameter.lifetime.ident == *ident,
            });
    }
}
