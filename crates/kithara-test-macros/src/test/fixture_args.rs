use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Attribute, Error, Expr, FnArg, Ident, ItemFn, Meta, Pat, Token};

pub(crate) enum ParamKind {
    Case,
    Future,
    Await,
    Fixture,
}

pub(crate) struct ParamInfo {
    pub(crate) ty: Box<syn::Type>,
    pub(crate) name: Ident,
    pub(crate) mutability: Option<Token![mut]>,
    pub(crate) kind: ParamKind,
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.first().is_some_and(|s| s.ident == name))
}

pub(crate) fn extract_params(func: &ItemFn) -> syn::Result<Vec<ParamInfo>> {
    func.sig
        .inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(pt) = arg else { return None };
            let Pat::Ident(pi) = &*pt.pat else {
                return None;
            };
            let kind = if has_attr(&pt.attrs, "case") {
                ParamKind::Case
            } else if let Some(attr) = pt.attrs.iter().find(|attr| attr.path().is_ident("future")) {
                match &attr.meta {
                    Meta::Path(_) => ParamKind::Future,
                    Meta::List(_)
                        if attr.parse_args::<Ident>().is_ok_and(|value| value == "awt") =>
                    {
                        ParamKind::Await
                    }
                    _ => {
                        return Some(Err(Error::new_spanned(
                            attr,
                            "expected #[future] or #[future(awt)]",
                        )));
                    }
                }
            } else {
                ParamKind::Fixture
            };
            Some(Ok(ParamInfo {
                kind,
                name: pi.ident.clone(),
                ty: pt.ty.clone(),
                mutability: pi.mutability,
            }))
        })
        .collect()
}

pub(crate) fn make_preamble(params: &[ParamInfo], case_values: Option<&[Expr]>) -> TokenStream2 {
    let mut stmts = Vec::new();
    let mut case_idx = 0;

    for p in params {
        let name = &p.name;
        let ty = &p.ty;
        let fn_name = {
            let s = name.to_string();
            let trimmed = s.trim_start_matches('_');
            if trimmed.is_empty() {
                name.clone()
            } else {
                format_ident!("{}", trimmed)
            }
        };
        let mutability = &p.mutability;
        match p.kind {
            ParamKind::Case => {
                if let Some(vals) = case_values
                    && let Some(val) = vals.get(case_idx)
                {
                    stmts.push(quote! { let #mutability #name: #ty = #val; });
                    case_idx += 1;
                }
            }
            ParamKind::Await => {
                stmts.push(quote! { let #mutability #name: #ty = #fn_name().await; });
            }
            ParamKind::Future => {
                stmts.push(quote! { let #mutability #name = #fn_name(); });
            }
            ParamKind::Fixture => {
                stmts.push(quote! { let #mutability #name: #ty = #fn_name(); });
            }
        }
    }

    quote! { #(#stmts)* }
}

#[cfg(test)]
mod tests {
    use super::{extract_params, make_preamble};

    #[test]
    fn awaited_fixture_is_ready_before_the_test_body() -> syn::Result<()> {
        let function = syn::parse_str("async fn test(#[future(awt)] media: Vec<u8>) {}")?;
        let params = extract_params(&function)?;
        let preamble = make_preamble(&params, None).to_string();
        assert!(preamble.contains("media () . await"), "{preamble}");
        Ok(())
    }
}
