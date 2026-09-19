use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    expand_inner(syn::parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_inner(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let Data::Enum(data) = input.data else {
        return Err(syn::Error::new_spanned(
            input.ident,
            "Variants requires an enum",
        ));
    };
    if let Some(variant) = data
        .variants
        .iter()
        .find(|variant| !matches!(variant.fields, Fields::Unit))
    {
        return Err(syn::Error::new_spanned(
            variant,
            "Variants supports only unit variants",
        ));
    }
    let name = input.ident;
    let variants = data.variants.iter().map(|variant| &variant.ident);
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            pub const ALL: &'static [Self] = &[#(Self::#variants),*];
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::expand_inner;

    #[kithara::test(native, flash(false))]
    fn refuses_payload_variants() {
        let input = syn::parse_quote!(
            enum Value {
                Unit,
                Payload(u8),
            }
        );
        assert!(expand_inner(input).is_err());
    }
}
