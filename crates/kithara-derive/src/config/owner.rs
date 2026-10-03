use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Ident, Token, Type,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

struct OwnerSpec {
    config: Option<Type>,
    path: Punctuated<Ident, Token![.]>,
}

impl Parse for OwnerSpec {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let config: Type = input.parse()?;
        let (config, path) = if input.is_empty() {
            let Type::Path(path) = config else {
                return Err(input.error("expected a named field"));
            };
            let Some(field) = path.path.get_ident() else {
                return Err(input.error("a nested field path needs an explicit config type"));
            };
            let mut fields = Punctuated::new();
            fields.push_value(field.clone());
            (None, fields)
        } else {
            input.parse::<Token![,]>()?;
            (Some(config), Punctuated::parse_separated_nonempty(input)?)
        };
        if !input.is_empty() {
            return Err(input.error("expected a field path after the configuration type"));
        }
        Ok(Self { config, path })
    }
}

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let Data::Struct(ref data) = input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "ConfigOwner can only be derived for a struct",
        ));
    };
    let Some(spec_attr) = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("config_owner"))
    else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "expected #[config_owner(field)] or #[config_owner(ConfigType, field.path)]",
        ));
    };
    let spec: OwnerSpec = spec_attr.parse_args()?;
    let Some(first) = spec.path.first() else {
        return Err(syn::Error::new_spanned(
            spec_attr,
            "configuration field path must not be empty",
        ));
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "ConfigOwner requires named fields",
        ));
    };
    let Some(field) = fields
        .named
        .iter()
        .find(|field| field.ident.as_ref() == Some(first))
    else {
        return Err(syn::Error::new_spanned(
            first,
            "configuration path must start with a field of this struct",
        ));
    };
    let name = &input.ident;
    let config = spec.config.as_ref().unwrap_or(&field.ty);
    let path = spec.path.iter();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mutable: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("config_owner_mut"))
        .collect();
    if mutable.len() > 1 {
        return Err(syn::Error::new_spanned(
            mutable[1],
            "duplicate config_owner_mut attribute",
        ));
    }
    let mutable_impl = mutable.first().map(|attribute| {
        if !matches!(&attribute.meta, syn::Meta::Path(_)) {
            return Err(syn::Error::new_spanned(
                attribute,
                "config_owner_mut takes no arguments",
            ));
        }
        let path = spec.path.iter();
        Ok(quote! {
            impl #impl_generics ::kithara_config::ConfigOwnerMut for #name #ty_generics #where_clause {
                fn config_mut(&mut self) -> &mut Self::Config {
                    &mut self.#(#path).*
                }
            }
        })
    }).transpose()?;
    Ok(quote! {
        impl #impl_generics ::kithara_config::ConfigOwner for #name #ty_generics #where_clause {
            type Config = #config;

            fn config(&self) -> &Self::Config {
                &self.#(#path).*
            }
        }
        #mutable_impl
    })
}
