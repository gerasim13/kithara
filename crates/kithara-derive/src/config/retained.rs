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
    let mut runtime_update = false;
    let mut values_vis = None;
    let mut seen: Vec<syn::Path> = Vec::new();
    syn::meta::parser(|meta| {
        if seen.contains(&meta.path) {
            return Err(meta.error("duplicate config option"));
        }
        seen.push(meta.path.clone());
        if meta.path.is_ident("default") {
            built_default = true;
        } else if meta.path.is_ident("update") {
            runtime_update = true;
        } else if meta.path.is_ident("builder") {
            builder = meta.value()?.parse::<syn::LitBool>()?.value;
        } else if meta.path.is_ident("values_vis") {
            let visibility: syn::LitStr = meta.value()?.parse()?;
            values_vis = Some(syn::parse_str::<syn::Visibility>(&visibility.value())?);
        } else {
            return Err(meta.error("expected default, update, builder = false, or values_vis"));
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
    let mut update_declarations: Vec<TokenStream> = Vec::new();
    let mut update_fields: Vec<TokenStream> = Vec::new();
    let mut update_lowers: Vec<TokenStream> = Vec::new();
    let name = &item.ident;
    for member in &mut fields.named {
        if let Some(expanded) = field::expand(member, &item.generics, name)? {
            value_fields.push(expanded.declaration);
            reads.push(expanded.read);
            if let Some(update) = expanded.update {
                update_declarations.push(update.declaration);
                update_fields.push(update.field);
                update_lowers.push(update.lower);
            }
        }
    }
    if !runtime_update && !update_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "field runtime updates require `#[config(update)]` on the struct",
        ));
    }
    if runtime_update && update_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "`#[config(update)]` requires at least one `#[config(value, update)]` field",
        ));
    }
    if runtime_update && !has_derive(&item.attrs, "Patch")? {
        return Err(syn::Error::new_spanned(
            name,
            "runtime updates require `#[derive(Patch)]` on the retained config",
        ));
    }
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
    let runtime = if runtime_update {
        runtime_updates(
            &item,
            visibility,
            &update_declarations,
            &update_fields,
            &update_lowers,
        )?
    } else {
        TokenStream::new()
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
    Ok(quote! { #item #snapshot #runtime })
}

fn has_derive(attributes: &[Attribute], expected: &str) -> Result<bool> {
    for attribute in attributes
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
    {
        let paths = attribute.parse_args_with(
            syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
        )?;
        if paths.iter().any(|path| {
            path.segments
                .last()
                .is_some_and(|segment| segment.ident == expected)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn runtime_updates(
    item: &ItemStruct,
    visibility: &syn::Visibility,
    declarations: &[TokenStream],
    fields: &[TokenStream],
    lowers: &[TokenStream],
) -> Result<TokenStream> {
    let name = &item.ident;
    let update = format_ident!("{name}Update");
    let patch = format_ident!("{name}Patch");
    let error = format_ident!("{name}PatchError");
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let gates = attributes(&item.attrs, false)?;
    #[cfg(feature = "patch")]
    let fallible = super::patch::is_fallible(&item.attrs, name.span())?;
    #[cfg(not(feature = "patch"))]
    let fallible = {
        return Err(syn::Error::new_spanned(
            name,
            "runtime updates require the kithara-derive `patch` feature",
        ));
    };
    let apply = if fallible {
        quote! {
            #visibility fn apply_update(
                &mut self,
                update: #update,
            ) -> ::core::result::Result<(), #error> {
                let mut patch = #patch::default();
                #(#lowers)*
                self.apply(patch)
            }
        }
    } else {
        quote! {
            #visibility fn apply_update(&mut self, update: #update) {
                let mut patch = #patch::default();
                #(#lowers)*
                self.apply(patch);
            }
        }
    };
    Ok(quote! {
        #(#declarations)*
        #(#gates)*
        #[derive(::core::default::Default)]
        #[non_exhaustive]
        #visibility struct #update {
            #(#fields,)*
        }
        #(#gates)*
        #[automatically_derived]
        impl #impl_generics #name #ty_generics #where_clause {
            #apply
        }
    })
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

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use quote::quote;

    use super::expand;

    #[kithara::test(native, flash(false))]
    fn duplicate_options_are_rejected_instead_of_overriding_configuration() {
        for options in [
            quote!(default, default),
            quote!(builder = false, builder = true),
            quote!(values_vis = "pub", values_vis = "pub(crate)"),
        ] {
            let error = expand(
                options,
                quote! {
                    struct Settings {
                        #[config(value)]
                        threshold: u32,
                    }
                },
            )
            .expect_err("duplicate configuration options must be rejected");
            assert_eq!(error.to_string(), "duplicate config option");
        }
    }

    #[kithara::test(native, flash(false))]
    #[cfg(feature = "patch")]
    fn optional_updates_emit_clear_and_only_declared_defaults_emit_reset() {
        let expanded = expand(
            quote!(default, update),
            quote! {
                #[derive(Clone, Patch)]
                struct Settings {
                    #[config(value, update)]
                    #[builder(required, default = Some(3))]
                    width: Option<usize>,
                    #[config(value, update)]
                    required: usize,
                }
            },
        )
        .expect("valid runtime update declaration")
        .to_string();

        assert!(expanded.contains("enum SettingsWidthUpdate"));
        assert!(expanded.contains("Clear"));
        assert!(expanded.contains("Reset"));
        assert!(expanded.contains("enum SettingsRequiredUpdate"));
        assert_eq!(
            expanded.matches("Reset").count(),
            2,
            "one variant and one lowering arm"
        );
        assert_eq!(
            expanded.matches("Clear").count(),
            2,
            "one variant and one lowering arm"
        );
    }

    #[kithara::test(native, flash(false))]
    #[cfg(feature = "patch")]
    fn update_rejects_non_value_roles_and_missing_struct_opt_in() {
        let nested = expand(
            quote!(update),
            quote! {
                struct Settings {
                    #[config(nested, update)]
                    nested: Nested,
                }
            },
        )
        .expect_err("nested updates need their own declared operation");
        assert_eq!(
            nested.to_string(),
            "runtime update currently requires a retained value field"
        );

        let missing = expand(
            quote!(),
            quote! {
                struct Settings {
                    #[config(value, update)]
                    value: usize,
                }
            },
        )
        .expect_err("field update requires struct opt-in");
        assert_eq!(
            missing.to_string(),
            "field runtime updates require `#[config(update)]` on the struct"
        );

        let patch = expand(
            quote!(update),
            quote! {
                struct Settings {
                    #[config(value, update)]
                    value: usize,
                }
            },
        )
        .expect_err("runtime lowering requires the existing Patch owner");
        assert_eq!(
            patch.to_string(),
            "runtime updates require `#[derive(Patch)]` on the retained config"
        );
    }
}
