use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, LitStr, Token, parse::Parse};

struct Options {
    all: Ident,
    method: Ident,
}

impl Parse for Options {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut all = None;
        let mut method = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: Ident = input.parse()?;
            match key.to_string().as_str() {
                "all" => all = Some(value),
                "method" => method = Some(value),
                _ => return Err(syn::Error::new_spanned(key, "expected `all` or `method`")),
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            all: all.ok_or_else(|| input.error("missing `all`"))?,
            method: method.ok_or_else(|| input.error("missing `method`"))?,
        })
    }
}

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    expand_inner(syn::parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_inner(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let options = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("enum_str"))
        .ok_or_else(|| syn::Error::new_spanned(&input.ident, "missing #[enum_str(...)]"))?
        .parse_args::<Options>()?;
    let Data::Enum(data) = input.data else {
        return Err(syn::Error::new_spanned(
            input.ident,
            "EnumStr requires an enum",
        ));
    };
    let name = input.ident;
    let all = options.all;
    let method = options.method;
    let names: Vec<_> = data
        .variants
        .iter()
        .map(|variant| LitStr::new(&variant.ident.to_string(), variant.ident.span()))
        .collect();
    let arms = data.variants.iter().zip(&names).map(|(variant, value)| {
        let ident = &variant.ident;
        let pattern = match variant.fields {
            Fields::Unit => quote!(Self::#ident),
            Fields::Unnamed(_) => quote!(Self::#ident(..)),
            Fields::Named(_) => quote!(Self::#ident { .. }),
        };
        quote!(#pattern => #value)
    });
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            pub const #all: &'static [&'static str] = &[#(#names),*];

            pub const fn #method(&self) -> &'static str {
                match self { #(#arms,)* }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::expand_inner;

    #[kithara::test(native, flash(false))]
    fn requires_explicit_output_names() {
        let input = syn::parse_quote!(
            enum Value {
                Unit,
            }
        );
        assert!(expand_inner(input).is_err());
    }
}
