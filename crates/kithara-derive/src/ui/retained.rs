use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Ident, Member, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn derive(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let mut setter = None;
    let mut field = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("retained") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("setter") {
                setter = Some(meta.value()?.parse::<Ident>()?);
                return Ok(());
            }
            if meta.path.is_ident("field") {
                field = Some(Member::Named(meta.value()?.parse::<Ident>()?));
                return Ok(());
            }
            Err(meta.error("unsupported retained option"))
        })?;
    }
    let set_read = setter.map(|setter| {
        let helper = setter;
        let data = field.map_or_else(|| quote!(data), |field| quote!(&mut data.#field));
        quote! {
            fn set_read(
                data: &mut Self::Data,
                value: &crate::render::ReadValue<'_>,
            ) -> bool {
                crate::render::masonry::controls::#helper(#data, value)
            }
        }
    });
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        #[cfg(feature = "masonry")]
        impl #impl_generics crate::render::masonry::controls::Retained
            for #name #type_generics #where_clause
        {
            #set_read
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use syn::parse_quote;

    use super::derive;

    #[kithara::test(native, flash(false))]
    fn emits_empty_and_projected_setters() {
        let empty = parse_quote!(
            struct Brand;
        );
        assert!(
            derive(&empty)
                .expect("valid derive")
                .to_string()
                .contains("Retained")
        );

        let projected = parse_quote! {
            #[retained(setter = set_bool, field = active)]
            struct Button;
        };
        let output = derive(&projected).expect("valid derive").to_string();
        assert!(output.contains("set_bool"));
        assert!(output.contains("data . active"));
    }
}
