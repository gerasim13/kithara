use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        impl #impl_generics crate::render::tree::mount::ViewControl for #name #type_generics #where_clause {
            fn view<'view>(
                &self,
                cx: &crate::render::tree::mount::Cx<'view, '_, '_>,
            ) -> crate::render::tree::geometry::Rendered<'view> {
                crate::render::tree::mount::painted(self, cx)
            }
        }
    }
    .into()
}
