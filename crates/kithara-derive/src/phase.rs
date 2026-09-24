#![cfg(feature = "phase")]

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, GenericParam, Path, Type, WherePredicate, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn derive(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let mut phase_trait = None;
    let mut sealed = None;
    let mut data = None;
    let mut generic = None;
    let mut bound = None;
    let mut track = None;
    let mut erase = None;
    let mut gat = false;

    for attr in &input.attrs {
        if !attr.path().is_ident("phase") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("trait") {
                phase_trait = Some(meta.value()?.parse::<Path>()?);
            } else if meta.path.is_ident("sealed") {
                sealed = Some(meta.value()?.parse::<Path>()?);
            } else if meta.path.is_ident("data") {
                data = Some(meta.value()?.parse::<Type>()?);
            } else if meta.path.is_ident("generic") {
                generic = Some(meta.value()?.parse::<GenericParam>()?);
            } else if meta.path.is_ident("bound") {
                bound = Some(meta.value()?.parse::<WherePredicate>()?);
            } else if meta.path.is_ident("track") {
                track = Some(meta.value()?.parse::<Path>()?);
            } else if meta.path.is_ident("erase") {
                erase = Some(meta.value()?.parse::<Path>()?);
            } else if meta.path.is_ident("gat") {
                gat = true;
            } else {
                return Err(meta.error("unsupported phase option"));
            }
            Ok(())
        })?;
    }

    let phase_trait =
        phase_trait.ok_or_else(|| Error::new_spanned(input, "missing phase trait"))?;
    let sealed = sealed.ok_or_else(|| Error::new_spanned(input, "missing sealed trait"))?;
    let data = data.ok_or_else(|| Error::new_spanned(input, "missing phase data"))?;
    let name = &input.ident;

    let sealed_impl = quote!(impl #sealed for #name {});
    let phase_impl = match (generic, bound, track, erase, gat) {
        (None, None, Some(track), Some(erase), false) => quote! {
            impl #phase_trait for #name {
                type Data = #data;

                fn erase(track: #track<Self>) -> CurrentFsm {
                    #erase(track)
                }
            }
        },
        (Some(generic), Some(bound), None, None, false) => quote! {
            impl<#generic> #phase_trait<#generic> for #name
            where
                #bound,
            {
                type Data = #data;
            }
        },
        (Some(generic), Some(bound), None, None, true) => quote! {
            impl #phase_trait for #name {
                type Data<#generic> = #data
                where
                    #bound;
            }
        },
        (None, None, None, None, false) => quote! {
            impl #phase_trait for #name {
                type Data = #data;
            }
        },
        _ => return Err(Error::new_spanned(input, "unsupported phase shape")),
    };

    Ok(quote! {
        #sealed_impl
        #phase_impl
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use syn::parse_quote;

    use super::derive;

    #[kithara::test(native, flash(false))]
    fn expands_track_phase() {
        let input = parse_quote! {
            #[phase(trait = TrackPhase, sealed = sealed::Sealed, data = (), track = Track, erase = CurrentFsm::Decoding)]
            struct Decoding;
        };
        let output = derive(&input).unwrap().to_string();
        assert!(output.contains("impl sealed :: Sealed for Decoding"));
        assert!(output.contains("CurrentFsm :: Decoding (track)"));
    }

    #[kithara::test(native, flash(false))]
    fn rejects_behavioral_shape() {
        let input = parse_quote! {
            #[phase(trait = Phase, sealed = sealed::Sealed, data = (), transition = next)]
            struct Active;
        };
        assert!(derive(&input).is_err());
    }
}
