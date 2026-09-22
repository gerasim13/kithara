use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Fields, Item, ItemStruct, Result, parse::Parser as _, parse_quote};

mod field;

pub(crate) fn expand(attributes: TokenStream, input: TokenStream) -> Result<TokenStream> {
    let item: Item = syn::parse2(input)?;
    match item {
        Item::Struct(item) => retained(attributes, item),
        Item::Impl(item) if attributes.is_empty() => Ok(quote! {
            #[::kithara_config::__private::bon::bon(crate = ::kithara_config::__private::bon)]
            #item
        }),
        Item::Fn(item) if attributes.is_empty() => Ok(quote! {
            #[::kithara_config::__private::bon::builder(crate = ::kithara_config::__private::bon)]
            #item
        }),
        other => Err(syn::Error::new_spanned(
            other,
            "config requires a named struct, or a function/impl without options",
        )),
    }
}

fn retained(options: TokenStream, mut item: ItemStruct) -> Result<TokenStream> {
    let mut built_default = false;
    let mut builder = true;
    let mut values_vis = None;
    syn::meta::parser(|meta| {
        if meta.path.is_ident("default") {
            built_default = true;
        } else if meta.path.is_ident("builder") {
            builder = meta.value()?.parse::<syn::LitBool>()?.value;
        } else if meta.path.is_ident("values_vis") {
            let visibility: syn::LitStr = meta.value()?.parse()?;
            values_vis = Some(syn::parse_str::<syn::Visibility>(&visibility.value())?);
        } else {
            return Err(meta.error("expected default, builder = false, or values_vis"));
        }
        Ok(())
    })
    .parse2(options)?;
    if built_default && !builder {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "default requires the struct builder",
        ));
    }
    let Fields::Named(fields) = &mut item.fields else {
        return Err(syn::Error::new_spanned(
            item,
            "config requires named fields",
        ));
    };
    let mut value_fields: Vec<TokenStream> = Vec::new();
    let mut reads: Vec<TokenStream> = Vec::new();
    for member in &mut fields.named {
        if let Some((declaration, read)) = field::expand(member, &item.generics)? {
            value_fields.push(declaration);
            reads.push(read);
        }
    }
    let name = &item.ident;
    let values = format_ident!("{name}Values");
    let visibility = values_vis.as_ref().unwrap_or(&item.vis);
    let gates = attributes(&item.attrs, false)?;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let snapshot = quote! {
        #(#gates)*
        #[doc = concat!("Owned readable values of `", stringify!(#name), "`. Resource inputs are excluded.")]
        #visibility struct #values {
            #(#value_fields,)*
        }
        #(#gates)*
        #[automatically_derived]
        impl #impl_generics ::kithara_config::Config for #name #ty_generics #where_clause {
            type Values = #values;
            fn values(&self) -> Self::Values {
                #values { #(#reads,)* }
            }
        }
    };
    if builder {
        item.attrs.insert(
            0,
            parse_quote!(#[derive(::kithara_config::__private::bon::Builder)]),
        );
        item.attrs
            .push(parse_quote!(#[builder(crate = ::kithara_config::__private::bon)]));
    }
    if built_default {
        item.attrs.insert(
            0,
            parse_quote!(#[derive(::kithara_config::__private::BuiltDefault)]),
        );
    }
    if !item
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("fieldwork"))
    {
        item.attrs.push(parse_quote!(#[fieldwork(opt_in, get)]));
    }
    item.attrs.insert(
        0,
        parse_quote!(#[derive(::kithara_config::__private::Fieldwork)]),
    );
    Ok(quote! { #item #snapshot })
}

fn attributes(input: &[Attribute], docs: bool) -> Result<Vec<Attribute>> {
    input
        .iter()
        .filter_map(|attr| match filter_meta(&attr.meta, docs) {
            Ok(Some(meta)) => Some(Ok(parse_quote!(#[#meta]))),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn filter_meta(meta: &syn::Meta, docs: bool) -> Result<Option<syn::Meta>> {
    if meta.path().is_ident("cfg") || (docs && meta.path().is_ident("doc")) {
        return Ok(Some(meta.clone()));
    }
    if let syn::Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
    {
        let arguments = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        let mut arguments = arguments.iter();
        let condition = arguments
            .next()
            .ok_or_else(|| syn::Error::new_spanned(list, "missing cfg_attr condition"))?;
        let mut kept: Vec<syn::Meta> = Vec::new();
        for argument in arguments {
            if let Some(meta) = filter_meta(argument, docs)? {
                kept.push(meta);
            }
        }
        if !kept.is_empty() {
            return Ok(Some(parse_quote!(cfg_attr(#condition, #(#kept),*))));
        }
    }
    Ok(None)
}
