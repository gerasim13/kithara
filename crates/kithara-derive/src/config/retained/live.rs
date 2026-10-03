use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, DeriveInput, Ident, Type};

use super::{
    control::{control, exec},
    field::{Member, upper_camel},
    implementation::docs,
};
use crate::config::field::Live;

/// `CheckedConfig` whenever the struct declares a check error or a live field,
/// then the change enum, `LiveConfig`, `<Name>Control` and `<Name>Exec` when it
/// has a live field.
pub(super) fn expand(
    item: &DeriveInput,
    members: &[Member<'_>],
    error: Option<&Type>,
) -> Option<TokenStream> {
    let fields: Vec<&Member<'_>> = members
        .iter()
        .filter(|member| member.live.is_some())
        .collect();
    if error.is_none() && fields.is_empty() {
        return None;
    }
    let checked = checked(item, members, error);
    let live = (!fields.is_empty()).then(|| {
        let change = live(item, &fields);
        let control = control(item, members);
        let exec = exec(item, &fields);
        quote! { #change #control #exec }
    });
    Some(quote! { #checked #live })
}

/// The `cfg` attributes of a field, which every statement generated from it
/// carries.
pub(super) fn cfgs(attributes: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("cfg"))
}

/// The change enum's name, spanned at the derive so generated items read as
/// the macro's own.
pub(super) fn change_name(item: &DeriveInput) -> Ident {
    format_ident!("{}Change", item.ident, span = Span::call_site())
}

/// The variant of the change enum that carries a change of `field`.
pub(super) fn variant(field: &Ident) -> Ident {
    Ident::new(&upper_camel(field), Span::call_site())
}

fn checked(item: &DeriveInput, members: &[Member<'_>], error: Option<&Type>) -> TokenStream {
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let error = error.map_or_else(
        || quote!(::core::convert::Infallible),
        |error| quote!(#error),
    );
    let steps: Vec<TokenStream> = members
        .iter()
        .filter_map(|member| {
            let field = member.name;
            let cfgs = cfgs(member.attributes);
            if member.nested {
                Some(quote! {
                    #(#cfgs)*
                    self.#field = ::kithara_config::CheckedConfig::validated(self.#field)?;
                })
            } else {
                member.check.as_ref().map(|check| {
                    quote! {
                        #(#cfgs)*
                        self.#field = #check(self.#field)?;
                    }
                })
            }
        })
        .collect();
    let receiver = if steps.is_empty() {
        quote!(self)
    } else {
        quote!(mut self)
    };
    quote! {
        #[automatically_derived]
        impl #impl_generics ::kithara_config::CheckedConfig for #name #ty_generics #where_clause {
            type Error = #error;

            fn validated(#receiver) -> ::core::result::Result<Self, Self::Error> {
                #(#steps)*
                ::core::result::Result::Ok(self)
            }
        }
    }
}

fn live(item: &DeriveInput, fields: &[&Member<'_>]) -> TokenStream {
    let name = &item.ident;
    let visibility = &item.vis;
    let change = change_name(item);
    let mut variants: Vec<TokenStream> = Vec::new();
    let mut checks: Vec<TokenStream> = Vec::new();
    let mut applies: Vec<TokenStream> = Vec::new();
    let mut nested: Vec<TokenStream> = Vec::new();
    for member in fields {
        let field = member.name;
        let ty = member.ty;
        let variant = variant(field);
        let payload = if member.nested {
            quote!(<#ty as ::kithara_config::LiveConfig>::Change)
        } else {
            quote!(#ty)
        };
        let surface = docs(member.attributes);
        let cfgs: Vec<&Attribute> = cfgs(member.attributes).collect();
        variants.push(quote! { #(#surface)* #variant(#payload) });
        if member.nested {
            checks.push(quote! {
                #(#cfgs)*
                #change::#variant(change) => ::core::result::Result::Ok(#change::#variant(
                    <#ty as ::kithara_config::LiveConfig>::check(change)?
                ))
            });
            applies.push(quote! {
                #(#cfgs)*
                #change::#variant(change) => ::kithara_config::LiveConfig::apply_change(&mut self.#field, change)
            });
            nested.push(quote! {
                #(#cfgs)*
                #[automatically_derived]
                impl ::core::convert::From<#payload> for #change {
                    fn from(change: #payload) -> Self {
                        Self::#variant(change)
                    }
                }
                #(#cfgs)*
                const _: () = ::core::assert!(
                    !<#ty as ::kithara_config::LiveConfig>::OWNER_FIELDS,
                    "a nested live config cannot have live(owner) fields"
                );
            });
        } else {
            let value = member
                .check
                .as_ref()
                .map_or_else(|| quote!(value), |check| quote!(#check(value)?));
            checks.push(quote! {
                #(#cfgs)*
                #change::#variant(value) => ::core::result::Result::Ok(#change::#variant(#value))
            });
            applies.push(quote! {
                #(#cfgs)*
                #change::#variant(value) => self.#field = value
            });
        }
    }
    let owner_fields = fields
        .iter()
        .any(|member| matches!(member.live, Some(Live::Owner)));
    let subject = format!(" One change of one live field of [`{name}`].");
    quote! {
        #[doc = #subject]
        #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug)]
        #visibility enum #change {
            #(#variants,)*
        }

        #(#nested)*

        #[automatically_derived]
        impl ::kithara_config::LiveConfig for #name {
            type Change = #change;

            const OWNER_FIELDS: bool = #owner_fields;

            fn apply_change(&mut self, change: Self::Change) {
                match change {
                    #(#applies,)*
                }
            }

            fn check(
                change: Self::Change,
            ) -> ::core::result::Result<Self::Change, <Self as ::kithara_config::CheckedConfig>::Error> {
                match change {
                    #(#checks,)*
                }
            }
        }
    }
}
