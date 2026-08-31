use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

use crate::{
    asset::parse::{AssetArgs, case_names},
    test::case::{extract_cases, is_case_attr},
};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as AssetArgs);
    let mut func = parse_macro_input!(item as ItemFn);

    let cases = match extract_cases(&func.attrs) {
        Ok(cases) => cases,
        Err(error) => return error.to_compile_error().into(),
    };
    let names = match case_names(&func.sig.ident, &cases) {
        Ok(names) => names,
        Err(error) => return error.to_compile_error().into(),
    };
    func.attrs.retain(|attr| !is_case_attr(attr));

    let fn_name = func.sig.ident.clone();
    let fn_name_literal = fn_name.to_string();
    let content_type = &args.content_type;
    let dependencies = &args.depends_on;
    let embed = args.embed;
    let ext = &args.ext;

    let submissions = names.iter().zip(&cases).map(|(case_literal, case)| {
        let values = &case.values;
        let call = if dependencies.is_empty() {
            quote! { #fn_name(#(#values),*) }
        } else {
            quote! { #fn_name(__inputs, #(#values),*) }
        };
        quote! {
            ::inventory::submit! {
                crate::registry::AssetDef {
                    build: |__inputs| #call,
                    case: #case_literal,
                    content_type: #content_type,
                    dependencies: &[#(#dependencies),*],
                    embed: #embed,
                    ext: #ext,
                    func: #fn_name_literal,
                }
            }
        }
    });

    quote! {
        #func
        #(#submissions)*
    }
    .into()
}
