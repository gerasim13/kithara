use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, Result};

pub(crate) fn derive(input: &DeriveInput) -> Result<TokenStream> {
    if matches!(input.data, Data::Union(_)) {
        return Err(Error::new_spanned(input, "`Event` needs a struct or enum"));
    }
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "`Event` needs a concrete type",
        ));
    }

    let ident = &input.ident;

    Ok(quote! {
        impl ::kithara_events::Event for #ident {}
    })
}

#[cfg(test)]
mod tests {
    use syn::{DeriveInput, parse_quote};

    use super::derive;

    fn expansion(input: &DeriveInput) -> String {
        derive(input).expect("the derive expands").to_string()
    }

    #[test]
    fn a_plain_struct_gets_one_empty_impl() {
        let input: DeriveInput = parse_quote! {
            struct FileEvent {
                bytes: u64,
            }
        };

        assert_eq!(
            expansion(&input),
            "impl :: kithara_events :: Event for FileEvent { }"
        );
    }

    #[test]
    fn an_enum_gets_one_empty_impl() {
        let input: DeriveInput = parse_quote! {
            enum PlayerEvent {
                Started,
                Stopped,
            }
        };

        assert_eq!(
            expansion(&input),
            "impl :: kithara_events :: Event for PlayerEvent { }"
        );
    }

    #[test]
    fn a_generic_type_is_refused() {
        let input: DeriveInput = parse_quote! {
            struct Wrapper<T> {
                inner: T,
            }
        };

        let error = derive(&input).expect_err("the derive refuses a type parameter");

        assert!(error.to_string().contains("needs a concrete type"));
    }

    #[test]
    fn a_lifetime_parameter_is_refused() {
        let input: DeriveInput = parse_quote! {
            struct Borrowed<'a> {
                inner: &'a str,
            }
        };

        let error = derive(&input).expect_err("the derive refuses a lifetime");

        assert!(error.to_string().contains("needs a concrete type"));
    }

    #[test]
    fn a_const_parameter_is_refused() {
        let input: DeriveInput = parse_quote! {
            struct Fixed<const N: usize> {
                inner: [u8; N],
            }
        };

        let error = derive(&input).expect_err("the derive refuses a const parameter");

        assert!(error.to_string().contains("needs a concrete type"));
    }

    #[test]
    fn a_union_is_refused() {
        let input: DeriveInput = parse_quote! {
            union Raw {
                bits: u64,
            }
        };

        let error = derive(&input).expect_err("the derive refuses a union");

        assert!(error.to_string().contains("needs a struct or enum"));
    }
}
