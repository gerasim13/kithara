use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Error, Expr, FnArg, Ident, ItemFn, Pat, PatIdent};

use super::parse::{ProbeEvent, ProbeFilter};

struct WireFields {
    fire_fn: Ident,
    arg_bindings: Vec<TokenStream2>,
    arg_consumes: Vec<TokenStream2>,
    computed_bindings: Vec<TokenStream2>,
    computed_consumes: Vec<TokenStream2>,
    slots: Vec<Ident>,
}

/// Collect every named parameter ident from a function signature.
/// Rejects patterns (`(a, b): _`, ref/mut bindings) — the probe
/// macro needs a bare ident to call `IntoProbeArg::into_probe_arg`
/// against, and pattern-bindings have no single name to use.
fn collect_fn_param_idents(input: &ItemFn) -> syn::Result<Vec<Ident>> {
    let mut idents = Vec::new();
    for arg in &input.sig.inputs {
        match arg {
            FnArg::Receiver(_) => {}
            FnArg::Typed(typed) => match typed.pat.as_ref() {
                Pat::Ident(PatIdent { ident, .. }) => idents.push(ident.clone()),
                other => {
                    return Err(Error::new_spanned(
                        other,
                        "#[kithara::probe] requires plain named arguments (no patterns)",
                    ));
                }
            },
        }
    }
    Ok(idents)
}

/// Resolve the optional `#[kithara::probe(a, b, …)]` filter against
/// the function's actual parameter list. `None` means "marker probe"
/// (no wire args). `Some(names)` must reference real parameters —
/// any ident that doesn't match is a hard error.
fn resolve_arg_idents(
    filter_args: Option<Vec<Ident>>,
    all_args: &[Ident],
) -> syn::Result<Vec<Ident>> {
    let Some(names) = filter_args else {
        return Ok(Vec::new());
    };
    if let Some(missing) = names
        .iter()
        .find(|name| !all_args.iter().any(|a| a == *name))
    {
        return Err(Error::new_spanned(
            missing,
            format!("#[kithara::probe(...)] arg `{missing}` does not match any function parameter"),
        ));
    }
    Ok(names)
}

fn wire_fields(
    args: &[Ident],
    computed: &[(Ident, Expr)],
    owner: &Ident,
) -> syn::Result<WireFields> {
    if let Some((name, _)) = computed
        .iter()
        .find(|(name, _)| args.iter().any(|arg| arg == name))
    {
        return Err(Error::new_spanned(
            name,
            format!("probe wire-name `{name}` is specified more than once"),
        ));
    }
    let total = args.len() + computed.len();
    if total > 6 {
        return Err(Error::new_spanned(
            owner,
            "probe supports at most 6 wire arguments (USDT provider arity ceiling)",
        ));
    }

    let arg_slots: Vec<Ident> = (0..args.len())
        .map(|index| format_ident!("__probe_arg_{index}"))
        .collect();
    let computed_slots: Vec<Ident> = (0..computed.len())
        .map(|index| format_ident!("__probe_computed_{index}"))
        .collect();
    let arg_bindings = args
        .iter()
        .zip(&arg_slots)
        .map(|(arg, slot)| {
            quote! {
                #[cfg(feature = "usdt")]
                let #slot: u64 =
                    ::kithara_test_utils::probe::IntoProbeArg::into_probe_arg(#arg);
            }
        })
        .collect();
    let computed_bindings = computed
        .iter()
        .zip(&computed_slots)
        .map(|((_, expression), slot)| {
            quote! {
                #[cfg(feature = "usdt")]
                let #slot: u64 =
                    ::kithara_test_utils::probe::IntoProbeArg::into_probe_arg(#expression);
            }
        })
        .collect();
    let arg_consumes = args.iter().map(|arg| quote! { let _ = &#arg; }).collect();
    let computed_consumes = computed
        .iter()
        .map(|(_, expression)| {
            quote! {
                if false {
                    let _ = #expression;
                }
            }
        })
        .collect();
    let slots = arg_slots.iter().chain(&computed_slots).cloned().collect();
    Ok(WireFields {
        arg_bindings,
        computed_bindings,
        arg_consumes,
        computed_consumes,
        slots,
        fire_fn: format_ident!("fire_{total}"),
    })
}

pub(crate) fn expand(input: &ItemFn, filter: ProbeFilter) -> syn::Result<TokenStream2> {
    let fn_name = input.sig.ident.clone();
    let fn_name_str = fn_name.to_string();

    let all_args = collect_fn_param_idents(input)?;
    let arg_idents = resolve_arg_idents(filter.args, &all_args)?;
    let computed = filter.computed;
    let probe_return = filter.probe_return;
    let fields = wire_fields(&arg_idents, &computed, &input.sig.ident)?;

    let attrs = &input.attrs;
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;
    let stmts = &block.stmts;

    let body = if probe_return {
        quote! {
            let __probe_ret = (|| #block)();
            #[cfg(feature = "usdt")]
            {
                ::kithara_test_utils::probe::register_probes();
                ::kithara_test_utils::probe::Probe::record_probe(&__probe_ret, #fn_name_str);
            }
            __probe_ret
        }
    } else {
        quote! { #(#stmts)* }
    };

    let emit_entry_event =
        build_emit_entry_event(probe_return, &fn_name_str, &fields.fire_fn, &fields.slots);
    let WireFields {
        arg_bindings,
        computed_bindings,
        arg_consumes,
        computed_consumes,
        ..
    } = fields;

    Ok(quote! {
        #(#attrs)*
        #vis #sig {
            #(#arg_consumes)*
            #(#computed_consumes)*
            #(#arg_bindings)*
            #(#computed_bindings)*
            #emit_entry_event
            #body
        }
    })
}

pub(crate) fn expand_event(event: ProbeEvent) -> syn::Result<TokenStream2> {
    let ProbeEvent {
        name,
        args,
        computed,
    } = event;
    let probe_name = name.to_string();
    let fields = wire_fields(&args, &computed, &name)?;
    let emit = build_emit_entry_event(false, &probe_name, &fields.fire_fn, &fields.slots);
    let WireFields {
        arg_bindings,
        computed_bindings,
        arg_consumes,
        computed_consumes,
        ..
    } = fields;
    Ok(quote! {{
        #(#arg_consumes)*
        #(#computed_consumes)*
        #(#arg_bindings)*
        #(#computed_bindings)*
        #emit
    }})
}

fn build_emit_entry_event(
    probe_return: bool,
    fn_name_str: &str,
    fire_fn: &Ident,
    probe_idents: &[Ident],
) -> TokenStream2 {
    if probe_return {
        return quote! {};
    }
    quote! {
        #[cfg(feature = "usdt")]
        {
            ::kithara_test_utils::probe::register_probes();
            ::kithara_test_utils::probe::#fire_fn(#fn_name_str, #(#probe_idents),*);
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::{Expr, ItemFn, Stmt, parse_quote};

    use super::*;

    #[test]
    fn non_return_probe_splices_original_tail_expression() -> syn::Result<()> {
        let input: ItemFn = parse_quote! {
            fn total_bytes(&self) -> u64 {
                self.layout.total_bytes()
            }
        };
        let filter = ProbeFilter {
            computed: vec![(parse_quote!(total), parse_quote!(self.layout.total_bytes()))],
            ..ProbeFilter::default()
        };

        let expanded: ItemFn = syn::parse2(expand(&input, filter)?)?;

        assert!(matches!(
            expanded.block.stmts.last(),
            Some(Stmt::Expr(Expr::MethodCall(call), None)) if call.method == "total_bytes"
        ));
        Ok(())
    }

    #[test]
    fn probe_emission_is_usdt_only() -> syn::Result<()> {
        let input: ItemFn = parse_quote! {
            fn advance(frames: u64) {
                let _ = frames;
            }
        };
        let filter = ProbeFilter {
            args: Some(vec![parse_quote!(frames)]),
            ..ProbeFilter::default()
        };

        let expanded = expand(&input, filter)?.to_string();

        assert!(expanded.contains("cfg (feature = \"usdt\")"));
        assert!(!expanded.contains("cfg (test)"));
        assert!(!expanded.contains("tracing"));
        assert!(!expanded.contains("rtsan"));
        Ok(())
    }
}
