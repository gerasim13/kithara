use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        #[cfg(feature = "masonry")]
        impl #impl_generics crate::render::masonry::mount::NodeControl
            for #name #type_generics #where_clause
        {
            fn leaf<Action>(
                &self,
                host: &crate::render::masonry::MasonryHost<'_, Action>,
                cx: &crate::render::masonry::mount::Cx<'_>,
            ) -> crate::render::masonry::MasonryNode<Action>
            where
                Action: std::fmt::Debug + Send + 'static,
            {
                crate::render::masonry::mount::painted(self, host, cx)
            }
        }
    }
    .into()
}
