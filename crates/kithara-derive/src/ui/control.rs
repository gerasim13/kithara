use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Expr, LitBool, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn derive(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let mut size = None;
    let mut composes_size = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("control") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("size") {
                size = Some(meta.value()?.parse::<Expr>()?);
                return Ok(());
            }
            if meta.path.is_ident("composes_size") {
                composes_size = Some(meta.value()?.parse::<LitBool>()?.value);
                return Ok(());
            }
            Err(meta.error("unsupported control option"))
        })?;
    }
    let size = size.ok_or_else(|| Error::new_spanned(input, "missing #[control(size = ...)]"))?;
    let composes_size = composes_size.map(|value| {
        quote! {
            fn composes_size(&self) -> bool {
                #value
            }
        }
    });
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics crate::mount::Control for #name #type_generics #where_clause {
            #composes_size

            fn size(&self, skin: &crate::skin::SkinDoc) -> crate::size::SizeSpec {
                #size
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use syn::parse_quote;

    use super::derive;

    #[kithara::test(native, flash(false))]
    fn emits_size_and_optional_composition_policy() {
        let input = parse_quote! {
            #[control(size = skin.cell.size, composes_size = false)]
            struct Cell;
        };
        let output = derive(&input).expect("valid derive").to_string();
        assert!(output.contains("skin . cell . size"));
        assert!(output.contains("fn composes_size"));
        assert!(output.contains("false"));
    }

    #[kithara::test(native, flash(false))]
    fn requires_a_size_expression() {
        let input = parse_quote!(
            struct Cell;
        );
        assert!(derive(&input).is_err());
    }
}
