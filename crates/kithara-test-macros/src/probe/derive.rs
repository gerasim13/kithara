use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DataStruct, DeriveInput, Error, Field, Fields, Ident, LitStr, Path, Type};

#[derive(Default)]
struct FieldOpts {
    rename: Option<String>,
    skip: bool,
}

fn parse_field_opts(field: &Field) -> syn::Result<FieldOpts> {
    let mut opts = FieldOpts::default();
    for attr in &field.attrs {
        if !attr.path().is_ident("probe") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                opts.skip = true;
                Ok(())
            } else if meta.path.is_ident("name") {
                let lit: LitStr = meta.value()?.parse()?;
                opts.rename = Some(syn::parse_str::<Ident>(&lit.value())?.to_string());
                Ok(())
            } else {
                Err(meta.error("unknown #[probe(...)] field option (expected `skip` or `name`)"))
            }
        })?;
    }
    Ok(opts)
}

pub(crate) fn expand_derive(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let crate_name = std::env::var("CARGO_PKG_NAME")
        .map_err(|_| Error::new_spanned(struct_name, "probe requires CARGO_PKG_NAME"))?
        .replace('-', "_");
    let target = format!("{crate_name}_probe");

    let fields = match &input.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(named),
            ..
        }) => &named.named,
        Data::Struct(_) => {
            return Err(Error::new_spanned(
                struct_name,
                "#[derive(Probe)] requires a struct with named fields",
            ));
        }
        _ => {
            return Err(Error::new_spanned(
                struct_name,
                "#[derive(Probe)] is only supported on structs",
            ));
        }
    };

    let mut field_idents: Vec<Ident> = Vec::new();
    let mut wire_names = Vec::new();
    for field in fields {
        let opts = parse_field_opts(field)?;
        if opts.skip {
            continue;
        }
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| Error::new_spanned(field, "expected named field"))?;
        wire_names.push(opts.rename.unwrap_or_else(|| ident.to_string()));
        field_idents.push(ident);
    }

    if field_idents.len() > 5 {
        return Err(Error::new_spanned(
            struct_name,
            "#[derive(Probe)] supports at most 5 payload fields (one of the \
             6 USDT provider slots is reserved for the operation id). Mark \
             extra fields with `#[probe(skip)]` or split the struct.",
        ));
    }
    let fire_fn = format_ident!("fire_{}", field_idents.len());

    let slot_idents: Vec<Ident> = (0..field_idents.len())
        .map(|i| format_ident!("__probe_slot_{}", i))
        .collect();

    let bindings: Vec<TokenStream2> = field_idents
        .iter()
        .zip(slot_idents.iter())
        .map(|(field, slot)| {
            quote! {
                let #slot: u64 = ::kithara_test_utils::probe::IntoProbeArg::into_probe_arg(self.#field);
            }
        })
        .collect();

    let tracing_pairs: Vec<TokenStream2> = wire_names
        .iter()
        .zip(&slot_idents)
        .map(|(name, slot)| {
            let ident = format_ident!("{name}");
            quote! { #ident = #slot }
        })
        .collect();

    let field_consume: Vec<TokenStream2> = field_idents
        .iter()
        .map(|f| quote! { let _ = &self.#f; })
        .collect();

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::kithara_test_utils::probe::Probe for #struct_name #ty_generics #where_clause {
            #[inline]
            fn record_probe(&self, name: &'static str, operation: u64) {
                #[cfg(feature = "usdt")]
                let __kithara_usdt_rtsan_permit = ::kithara_test_utils::rtsan::permit();
                let _ = (name, operation);
                #(#field_consume)*
                #[cfg(all(feature = "usdt", target_os = "macos", not(miri)))]
                {
                    ::kithara_test_utils::probe::register_probes();
                    #(#bindings)*
                    ::kithara_test_utils::probe::#fire_fn(operation, #(#slot_idents),*);
                }
                #[cfg(feature = "usdt")]
                {
                    #(#bindings)*
                    ::kithara_test_utils::tracing::event!(
                        target: #target,
                        ::kithara_test_utils::tracing::Level::TRACE,
                        probe = name,
                        #(#tracing_pairs),*
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use syn::{DeriveInput, parse_quote};

    use super::{expand_derive, expand_derive_into_probe_arg};

    #[test]
    fn derive_uses_only_usdt_platform_backends() -> syn::Result<()> {
        let input: DeriveInput = parse_quote! {
            struct Sample {
                #[probe(name = "frames")]
                samples: u64,
            }
        };

        let expanded = expand_derive(&input)?.to_string();

        assert!(
            expanded
                .contains("cfg (all (feature = \"usdt\" , target_os = \"macos\" , not (miri)))")
        );
        assert!(expanded.contains("cfg (feature = \"usdt\")"));
        assert!(expanded.contains("kithara_test_utils :: tracing :: event"));
        assert!(expanded.contains("frames = __probe_slot_0"));
        assert!(!expanded.contains("cfg (test)"));
        assert!(!expanded.contains("probe-capture"));
        assert!(expanded.contains("rtsan :: permit"));
        Ok(())
    }

    #[test]
    fn derive_rejects_a_non_identifier_probe_name() {
        let input: DeriveInput = parse_quote! {
            struct Sample {
                #[probe(name = "not-a-field")]
                samples: u64,
            }
        };

        let err = expand_derive(&input).expect_err("invalid tracing field name");

        assert!(!err.to_string().is_empty(), "{err}");
    }

    #[test]
    fn into_probe_arg_encodes_unit_enums_without_reverse_conversion() -> syn::Result<()> {
        let input: DeriveInput = parse_quote! {
            #[probe_arg(encode_only)]
            enum Priority { High = 0, Low = 1 }
        };

        let expanded = expand_derive_into_probe_arg(&input)?.to_string();

        assert!(expanded.contains("self as u64"));
        assert!(!expanded.contains("from_probe_arg"));
        Ok(())
    }

    #[test]
    fn into_probe_arg_encodes_non_zero_newtypes_without_reverse_conversion() -> syn::Result<()> {
        let input: DeriveInput = parse_quote! {
            #[probe_arg(encode_only)]
            struct RequestId(::std::num::NonZeroU64);
        };

        let expanded = expand_derive_into_probe_arg(&input)?.to_string();

        assert!(expanded.contains("u64 :: from (self . 0 . get ())"));
        assert!(!expanded.contains("from_probe_arg"));
        Ok(())
    }

    #[test]
    fn into_probe_arg_projects_owned_and_borrowed_values() -> syn::Result<()> {
        let owned: DeriveInput = parse_quote! {
            #[probe_arg(encode_only, with = Self::encode_probe_arg)]
            enum Mode { Auto, Manual }
        };
        let borrowed: DeriveInput = parse_quote! {
            #[probe_arg(encode_only, by_ref, with = Self::encode_probe_arg)]
            enum Decision { Stay, Switch }
        };

        let owned = expand_derive_into_probe_arg(&owned)?.to_string();
        let borrowed = expand_derive_into_probe_arg(&borrowed)?.to_string();

        assert!(owned.contains("Mode :: encode_probe_arg (self)"));
        assert!(borrowed.contains("for & Decision"));
        assert!(borrowed.contains("Decision :: encode_probe_arg (self)"));
        assert!(!owned.contains("cfg"));
        assert!(!borrowed.contains("cfg"));
        Ok(())
    }

    #[test]
    fn into_probe_arg_refuses_unsafe_reverse_conversions() {
        let unit_enum: DeriveInput = parse_quote! { enum Priority { High, Low } };
        let projection: DeriveInput = parse_quote! {
            #[probe_arg(with = Self::encode_probe_arg)]
            enum Mode { Auto, Manual }
        };

        assert!(expand_derive_into_probe_arg(&unit_enum).is_err());
        assert!(expand_derive_into_probe_arg(&projection).is_err());
    }
}

/// Expand `#[derive(kithara::IntoProbeArg)]` for a single-field
/// `Copy` newtype struct. Generates round-trippable `into_probe_arg`
/// and `from_probe_arg` impls that delegate to the inner field's own
/// `IntoProbeArg` impl. Multi-field structs and enums are rejected:
/// they need an explicit packed impl with a documented bit layout
/// (`SegmentRequest::into_probe_arg` is the canonical example).
pub(crate) fn expand_derive_into_probe_arg(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let options = ProbeArgOptions::parse(input)?;
    if let Some(mut encoder) = options.with {
        if !options.encode_only {
            return Err(Error::new_spanned(
                struct_name,
                "`with` requires `encode_only`: a projection cannot synthesize a reverse value",
            ));
        }
        if let Some(first) = encoder.segments.first_mut()
            && first.ident == "Self"
        {
            first.ident = struct_name.clone();
        }
        let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
        let target = if options.by_ref {
            quote!(&#struct_name #ty_generics)
        } else {
            quote!(#struct_name #ty_generics)
        };
        return Ok(quote! {
            impl #impl_generics ::kithara_test_utils::probe::IntoProbeArg
            for #target #where_clause {
                fn into_probe_arg(self) -> u64 {
                    #encoder(self)
                }
            }
        });
    }

    if options.by_ref {
        return Err(Error::new_spanned(
            struct_name,
            "`by_ref` requires `with = <path>`",
        ));
    }

    if let Data::Enum(data) = &input.data {
        if !options.encode_only {
            return Err(Error::new_spanned(
                struct_name,
                "unit enums require `#[probe_arg(encode_only)]`: invalid tags cannot be decoded safely",
            ));
        }
        if data
            .variants
            .iter()
            .any(|variant| !matches!(variant.fields, Fields::Unit))
        {
            return Err(Error::new_spanned(
                struct_name,
                "encode-only enum variants must carry no fields; use `with = <path>` for projections",
            ));
        }
        let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
        return Ok(quote! {
            impl #impl_generics ::kithara_test_utils::probe::IntoProbeArg
            for #struct_name #ty_generics #where_clause {
                fn into_probe_arg(self) -> u64 {
                    self as u64
                }
            }
        });
    }

    let Data::Struct(DataStruct { fields, .. }) = &input.data else {
        return Err(Error::new_spanned(
            struct_name,
            "#[derive(IntoProbeArg)] is only supported on structs",
        ));
    };

    let (field_access, field_ty, ctor): (TokenStream2, Type, TokenStream2) = match fields {
        Fields::Unit => {
            return Err(Error::new_spanned(
                struct_name,
                "#[derive(IntoProbeArg)] requires exactly one field — \
                 unit structs carry no probe payload",
            ));
        }
        Fields::Unnamed(unnamed) => {
            if unnamed.unnamed.len() != 1 {
                return Err(Error::new_spanned(
                    struct_name,
                    "#[derive(IntoProbeArg)] requires exactly one tuple field. \
                     Multi-field structs need an explicit packed impl with a \
                     documented bit layout (see `SegmentRequest`).",
                ));
            }
            let field = unnamed
                .unnamed
                .first()
                .expect("invariant: checked len == 1 above");
            let ty = field.ty.clone();
            (
                quote!(self.0),
                ty.clone(),
                quote!(Self(<#ty as ::kithara_test_utils::probe::IntoProbeArg>::from_probe_arg(packed))),
            )
        }
        Fields::Named(named) => {
            if named.named.len() != 1 {
                return Err(Error::new_spanned(
                    struct_name,
                    "#[derive(IntoProbeArg)] requires exactly one named field. \
                     Multi-field structs need an explicit packed impl with a \
                     documented bit layout (see `SegmentRequest`).",
                ));
            }
            let field = named
                .named
                .first()
                .expect("invariant: checked len == 1 above");
            let name = field
                .ident
                .as_ref()
                .ok_or_else(|| Error::new_spanned(field, "expected named field"))?;
            let ty = field.ty.clone();
            (
                quote!(self.#name),
                ty.clone(),
                quote!(Self {
                    #name: <#ty as ::kithara_test_utils::probe::IntoProbeArg>::from_probe_arg(packed),
                }),
            )
        }
    };

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let encode = if options.encode_only && is_non_zero_u64(&field_ty) {
        quote!(u64::from(#field_access.get()))
    } else {
        quote!(<#field_ty as ::kithara_test_utils::probe::IntoProbeArg>::into_probe_arg(#field_access))
    };
    let decode = (!options.encode_only).then(|| {
        quote! {
            fn from_probe_arg(packed: u64) -> Self {
                #ctor
            }
        }
    });

    Ok(quote! {
        impl #impl_generics ::kithara_test_utils::probe::IntoProbeArg
        for #struct_name #ty_generics #where_clause {
            fn into_probe_arg(self) -> u64 {
                #encode
            }
            #decode
        }
    })
}

#[derive(Default)]
struct ProbeArgOptions {
    by_ref: bool,
    encode_only: bool,
    with: Option<Path>,
}

impl ProbeArgOptions {
    fn parse(input: &DeriveInput) -> syn::Result<Self> {
        let mut options = Self::default();
        for attribute in input
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("probe_arg"))
        {
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("by_ref") {
                    if options.by_ref {
                        return Err(meta.error("duplicate `by_ref` option"));
                    }
                    options.by_ref = true;
                } else if meta.path.is_ident("encode_only") {
                    if options.encode_only {
                        return Err(meta.error("duplicate `encode_only` option"));
                    }
                    options.encode_only = true;
                } else if meta.path.is_ident("with") {
                    if options.with.is_some() {
                        return Err(meta.error("duplicate `with` option"));
                    }
                    options.with = Some(meta.value()?.parse()?);
                } else {
                    return Err(meta.error("expected `encode_only`, `by_ref`, or `with = <path>`"));
                }
                Ok(())
            })?;
        }
        Ok(options)
    }
}

fn is_non_zero_u64(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "NonZeroU64"))
}
