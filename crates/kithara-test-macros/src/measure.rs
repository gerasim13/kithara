use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_tokens(&attr.into(), &item.into()).into()
}

fn expand_tokens(attr: &TokenStream2, item: &TokenStream2) -> TokenStream2 {
    if attr.is_empty() {
        quote! {
            #[cfg_attr(feature = "perf", hotpath::measure)]
            #item
        }
    } else {
        quote! {
            #[cfg_attr(feature = "perf", hotpath::measure(#attr))]
            #item
        }
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::expand_tokens;

    #[test]
    fn gates_bare_and_labeled_measurements() {
        let bare = expand_tokens(
            &quote!(),
            &quote!(
                fn read() {}
            ),
        )
        .to_string();
        let labeled = expand_tokens(
            &quote!(label = "audio.read"),
            &quote!(
                fn read() {}
            ),
        )
        .to_string();

        for expanded in [&bare, &labeled] {
            assert!(expanded.contains("feature = \"perf\""));
            assert!(expanded.contains("hotpath :: measure"));
        }
        assert!(labeled.contains("label = \"audio.read\""));
    }
}
