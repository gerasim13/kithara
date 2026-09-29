use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, Result, Visibility, meta::ParseNestedMeta,
    parenthesized,
};

use super::field::{self, Construction, Member};
#[cfg(feature = "patch")]
use crate::config::patch::{Check, validation};

/// What `#[config(...)]` on the type itself declares.
#[derive(Default)]
struct Options {
    built_default: bool,
    construction: bool,
    runtime_update: bool,
    sdk: bool,
    debug: bool,
    values_vis: Option<Visibility>,
    /// bon's top-level options for the generated builder.
    builder: Option<TokenStream>,
}

impl Options {
    fn parse(item: &DeriveInput) -> Result<Self> {
        let mut options = Self::default();
        let mut seen: Vec<syn::Path> = Vec::new();
        for attr in item
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("config"))
        {
            attr.parse_nested_meta(|meta| {
                if seen.contains(&meta.path) {
                    return Err(meta.error("duplicate config option"));
                }
                seen.push(meta.path.clone());
                if meta.path.is_ident("default") {
                    options.built_default = true;
                } else if meta.path.is_ident("construction") {
                    options.construction = true;
                } else if meta.path.is_ident("update") {
                    options.runtime_update = true;
                } else if meta.path.is_ident("sdk") {
                    options.sdk = true;
                } else if meta.path.is_ident("debug") {
                    options.debug = true;
                } else if meta.path.is_ident("builder") {
                    options.builder = Some(group(&meta)?);
                } else if meta.path.is_ident("patch") {
                    // `Patch` reads this group; the update gate below reads its check.
                    group(&meta)?;
                } else if meta.path.is_ident("values_vis") {
                    let visibility: syn::LitStr = meta.value()?.parse()?;
                    options.values_vis = Some(syn::parse_str(&visibility.value())?);
                } else {
                    return Err(meta.error(
                        "expected construction, default, update, sdk, debug, builder(...), \
                         patch(...), or values_vis",
                    ));
                }
                Ok(())
            })?;
        }
        if options.construction
            && (options.built_default
                || options.runtime_update
                || options.sdk
                || options.values_vis.is_some())
        {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "construction inputs cannot declare retained defaults, updates, SDK records, or values visibility",
            ));
        }
        Ok(options)
    }
}

/// The tokens inside one `name(...)` group of `#[config(...)]`.
pub(super) fn group(meta: &ParseNestedMeta<'_>) -> Result<TokenStream> {
    let content;
    parenthesized!(content in meta.input);
    let tokens: TokenStream = content.parse()?;
    if tokens.is_empty() {
        return Err(meta.error("empty config group"));
    }
    Ok(tokens)
}

/// The `doc` and `cfg` attributes of a field, which every item generated from
/// it carries.
pub(super) fn docs(attributes: &[Attribute]) -> Vec<&Attribute> {
    attributes
        .iter()
        .filter(|attr| attr.path().is_ident("doc") || attr.path().is_ident("cfg"))
        .collect()
}

pub(crate) fn expand(input: TokenStream) -> Result<TokenStream> {
    let item: DeriveInput = syn::parse2(input)?;
    let options = Options::parse(&item)?;
    let Data::Struct(data) = &item.data else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "config requires a struct with named fields",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "config requires named fields",
        ));
    };
    let members = fields
        .named
        .iter()
        .map(|field| field::expand(field, &item, !options.construction))
        .collect::<Result<Vec<_>>>()?;
    if !options.debug
        && let Some(member) = members.iter().find(|member| !member.debugged)
    {
        return Err(syn::Error::new_spanned(
            member.name,
            "debug(skip) requires `#[config(debug)]` on the type",
        ));
    }
    let builder = builder(&item, &options, &members);
    let accessors = accessors(&item, &members);
    let default = options.built_default.then(|| built_default(&item));
    let debug = options.debug.then(|| debug(&item, &members));
    let snapshot = (!options.construction)
        .then(|| snapshot(&item, &options, &members))
        .transpose()?;
    Ok(quote! { #builder #accessors #default #debug #snapshot })
}

/// A bon function builder over `new`, which bon keeps private and hidden:
/// the type's constructor is `X::builder()`. Skipped fields are bound in
/// declaration order before any argument moves into `Self`, so their
/// expressions read the arguments and the skipped fields above them.
fn builder(item: &DeriveInput, options: &Options, members: &[Member<'_>]) -> TokenStream {
    let name = &item.ident;
    let visibility = &item.vis;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let top = options
        .builder
        .as_ref()
        .map_or_else(|| quote!(#[builder]), |group| quote!(#[builder(#group)]));
    let arguments = members
        .iter()
        .filter_map(|member| match &member.construction {
            Construction::Argument(argument) => Some(argument),
            Construction::Initialised(_) => None,
        });
    let initialisers = members
        .iter()
        .filter_map(|member| match &member.construction {
            Construction::Argument(_) => None,
            Construction::Initialised(value) => {
                let (field, ty) = (member.name, member.ty);
                Some(quote!(let #field: #ty = #value;))
            }
        });
    let fields = members.iter().map(|member| member.name);
    quote! {
        #[::kithara_config::__private::bon::bon(crate = ::kithara_config::__private::bon)]
        #[automatically_derived]
        impl #impl_generics #name #ty_generics #where_clause {
            #top
            #visibility fn new(#(#arguments),*) -> Self {
                #(#initialisers)*
                Self { #(#fields),* }
            }
        }
    }
}

fn accessors(item: &DeriveInput, members: &[Member<'_>]) -> Option<TokenStream> {
    let accessors: Vec<&TokenStream> = members
        .iter()
        .filter_map(|member| member.accessor.as_ref())
        .collect();
    if accessors.is_empty() {
        return None;
    }
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    Some(quote! {
        #[automatically_derived]
        impl #impl_generics #name #ty_generics #where_clause {
            #(#accessors)*
        }
    })
}

fn built_default(item: &DeriveInput) -> TokenStream {
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    quote! {
        #[automatically_derived]
        impl #impl_generics ::core::default::Default for #name #ty_generics #where_clause {
            fn default() -> Self {
                Self::builder().build()
            }
        }
    }
}

/// `Debug` over the fields not marked `debug(skip)`; a debugged field whose
/// type names a type parameter bounds that type, not the parameter.
fn debug(item: &DeriveInput, members: &[Member<'_>]) -> TokenStream {
    let name = &item.ident;
    let (impl_generics, ty_generics, _) = item.generics.split_for_impl();
    let mut generics = item.generics.clone();
    let predicates = &mut generics.make_where_clause().predicates;
    let mut fields: Vec<TokenStream> = Vec::new();
    for member in members.iter().filter(|member| member.debugged) {
        let field = member.name;
        let ty = member.ty;
        if field::names_a_parameter(ty, &item.generics) {
            predicates.push(syn::parse_quote!(#ty: ::core::fmt::Debug));
        }
        fields.push(quote!(.field(::core::stringify!(#field), &self.#field)));
    }
    let finish = if fields.len() == members.len() {
        quote!(finish)
    } else {
        quote!(finish_non_exhaustive)
    };
    let where_clause = &generics.where_clause;
    quote! {
        #[automatically_derived]
        impl #impl_generics ::core::fmt::Debug for #name #ty_generics #where_clause {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_struct(::core::stringify!(#name))
                    #(#fields)*
                    .#finish()
            }
        }
    }
}

fn snapshot(item: &DeriveInput, options: &Options, members: &[Member<'_>]) -> Result<TokenStream> {
    let name = &item.ident;
    let mut value_fields: Vec<&TokenStream> = Vec::new();
    let mut reads: Vec<&TokenStream> = Vec::new();
    let mut update_declarations: Vec<&TokenStream> = Vec::new();
    let mut update_fields: Vec<&TokenStream> = Vec::new();
    let mut update_lowers: Vec<&TokenStream> = Vec::new();
    for retained in members.iter().filter_map(|member| member.retained.as_ref()) {
        value_fields.push(&retained.declaration);
        reads.push(&retained.read);
        if let Some(update) = &retained.update {
            update_declarations.push(&update.declaration);
            update_fields.push(&update.field);
            update_lowers.push(&update.lower);
        }
    }
    if !options.runtime_update && !update_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "field runtime updates require `#[config(update)]` on the struct",
        ));
    }
    if options.runtime_update && update_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "`#[config(update)]` requires at least one `#[config(value, update)]` field",
        ));
    }
    let values = format_ident!("{name}Values");
    let visibility = options.values_vis.as_ref().unwrap_or(&item.vis);
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let runtime = if options.runtime_update {
        let update = format_ident!("{name}Update");
        let apply = apply_update(item, visibility, &update, &update_lowers)?;
        quote! {
            #(#update_declarations)*
            #[derive(::core::default::Default)]
            #[non_exhaustive]
            #visibility struct #update {
                #(#update_fields,)*
            }
            #[automatically_derived]
            impl #impl_generics #name #ty_generics #where_clause {
                #apply
            }
        }
    } else {
        TokenStream::new()
    };
    Ok(quote! {
        #[doc = concat!("Owned readable values of `", stringify!(#name), "`. Resource inputs are excluded.")]
        #visibility struct #values {
            #(#value_fields,)*
        }
        #[automatically_derived]
        impl #impl_generics ::kithara_config::Config for #name #ty_generics #where_clause {
            type Values = #values;
            fn values(&self) -> Self::Values {
                #values { #(#reads,)* }
            }
        }
        #runtime
    })
}

/// Every update lowers onto `target`. A configuration that judges itself
/// stages the change and commits only what its declared check accepts, the
/// same gate a document merge holds; any other takes the change in place.
#[cfg(feature = "patch")]
fn apply_update(
    item: &DeriveInput,
    visibility: &Visibility,
    update: &Ident,
    lowers: &[&TokenStream],
) -> Result<TokenStream> {
    let Some(Check { with, error }) = validation(&item.attrs, item.ident.span())? else {
        return Ok(quote! {
            #visibility fn apply_update(&mut self, update: #update) {
                let target = self;
                #(#lowers)*
            }
        });
    };
    Ok(quote! {
        #visibility fn apply_update(
            &mut self,
            update: #update,
        ) -> ::core::result::Result<(), #error> {
            let mut staged = ::core::clone::Clone::clone(&*self);
            let target = &mut staged;
            #(#lowers)*
            *self = #with(staged)?;
            ::core::result::Result::Ok(())
        }
    })
}

#[cfg(not(feature = "patch"))]
fn apply_update(
    item: &DeriveInput,
    _: &Visibility,
    _: &Ident,
    _: &[&TokenStream],
) -> Result<TokenStream> {
    Err(syn::Error::new_spanned(
        &item.ident,
        "runtime updates require the kithara-derive `patch` feature",
    ))
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use proc_macro2::TokenStream;
    use quote::quote;

    use super::expand;

    fn expansion(input: TokenStream) -> String {
        expand(input)
            .expect("a valid configuration expands")
            .to_string()
    }

    fn refusal(input: TokenStream) -> String {
        expand(input)
            .expect_err("an invalid configuration is refused")
            .to_string()
    }

    #[kithara::test(native, flash(false))]
    fn duplicate_options_are_rejected_instead_of_overriding_configuration() {
        for options in [
            quote!(default, default),
            quote!(debug, debug),
            quote!(builder(on(String, into)), builder(on(u32, into))),
            quote!(values_vis = "pub", values_vis = "pub(crate)"),
        ] {
            assert_eq!(
                refusal(quote! {
                    #[config(#options)]
                    struct Settings {
                        #[config(value)]
                        threshold: u32,
                    }
                }),
                "duplicate config option"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn the_builder_is_a_bon_function_over_every_field_but_the_skipped_ones() {
        let expanded = expansion(quote! {
            #[config(builder(state_mod(vis = "pub")))]
            pub struct Settings {
                /// How far the ratio may go.
                #[config(value, builder(default = Consts::MAX_BAR_RATIO))]
                ratio: f64,
                #[config(skip = "set while running", builder(skip = Phase::Idle))]
                phase: Phase,
                #[config(skip = "counted while running", builder(skip))]
                count: usize,
            }
        });

        assert!(expanded.contains("# [builder (state_mod (vis = \"pub\"))] pub fn new"));
        assert!(expanded.contains(
            "# [doc = r\" How far the ratio may go.\"] # [builder (default = Consts :: MAX_BAR_RATIO)] ratio : f64"
        ));
        assert!(expanded.contains(
            "let phase : Phase = Phase :: Idle ; \
             let count : usize = :: core :: default :: Default :: default () ; \
             Self { ratio , phase , count }"
        ));
        assert!(
            !expanded.contains("phase : Phase)"),
            "a skipped field is no argument"
        );
    }

    #[kithara::test(native, flash(false))]
    fn accessors_return_a_reference_or_a_copy() {
        let expanded = expansion(quote! {
            pub(crate) struct Settings {
                #[config(value, field(get))]
                name: String,
                #[config(value, field(get, copy))]
                ratio: f64,
            }
        });

        assert!(expanded.contains("pub (crate) fn name (& self) -> & String { & self . name }"));
        assert!(expanded.contains("pub (crate) fn ratio (& self) -> f64 { self . ratio }"));
    }

    #[kithara::test(native, flash(false))]
    fn debug_omits_skipped_fields_and_bounds_generic_field_types() {
        let expanded = expansion(quote! {
            #[config(debug)]
            struct Player<S> {
                #[config(skip = "injected worker")]
                worker: Worker<S>,
                #[config(skip = "injected bus", debug(skip))]
                bus: Bus,
                #[config(value)]
                volume: f32,
            }
        });

        assert!(expanded.contains("Worker < S > : :: core :: fmt :: Debug"));
        assert!(!expanded.contains("S : :: core :: fmt :: Debug"));
        assert!(expanded.contains(". field (:: core :: stringify ! (volume) , & self . volume)"));
        assert!(!expanded.contains("stringify ! (bus)"));
        assert!(expanded.contains(". finish_non_exhaustive ()"));
    }

    #[kithara::test(native, flash(false))]
    fn a_wrapped_field_takes_its_wire_value_through_the_builder() {
        let expanded = expansion(quote! {
            struct Settings {
                #[config(value(f32, self.fade.load()), wrap(default = 1.5, with = Atomic::new))]
                fade: Atomic,
            }
        });

        assert!(expanded.contains(
            "# [builder (default = Atomic :: new (1.5) , with = | value : f32 | Atomic :: new (value))] fade : Atomic"
        ));
    }

    #[kithara::test(native, flash(false))]
    fn construction_inputs_have_a_builder_without_a_retained_snapshot() {
        let expanded = expansion(quote! {
            #[config(construction)]
            struct Input<T> {
                #[config(skip = "injected resource", builder(start_fn))]
                resource: T,
                #[config(value, builder(default), field(get, copy))]
                capacity: usize,
            }
        });

        assert!(expanded.contains("# [builder (start_fn)] resource : T"));
        assert!(expanded.contains("fn capacity (& self) -> usize"));
        assert!(!expanded.contains("InputValues"));
        assert!(!expanded.contains("kithara_config :: Config for Input"));
    }

    #[kithara::test(native, flash(false))]
    fn construction_inputs_reject_retained_only_options() {
        for options in [
            quote!(construction, default),
            quote!(construction, update),
            quote!(construction, sdk),
            quote!(construction, values_vis = "pub"),
        ] {
            assert!(
                expand(quote! {
                    #[config(#options)]
                    struct Input {
                        #[config(value)]
                        capacity: usize,
                    }
                })
                .is_err(),
                "construction accepted retained-only options: {options}"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn malformed_field_declarations_are_rejected() {
        for field in [
            quote!(#[config(value, builder(default), builder(required))] field: u32),
            quote!(#[config(value, builder())] field: u32),
            quote!(#[config(value, builder(skip, default))] field: u32),
            quote!(#[config(value, field)] field: u32),
            quote!(#[config(value, field(set))] field: u32),
            quote!(#[config(value, field(copy))] field: u32),
            quote!(#[config(value, debug(show))] field: u32),
            quote!(#[config(value(u32, self.field.0), wrap(default = 1))] field: Wrapped),
            quote!(#[config(value, wrap(default = 1, with = Wrapped::new))] field: Wrapped),
            quote!(#[config(value(u32, self.field.0), wrap(default = 1, with = Wrapped::new), builder(required))] field: Wrapped),
            quote!(field: u32),
        ] {
            assert!(
                expand(quote!(#[config(debug)] struct Settings { #field })).is_err(),
                "malformed field declaration was accepted: {field}"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn debug_skip_requires_a_generated_debug() {
        assert_eq!(
            refusal(quote! {
                struct Settings {
                    #[config(skip = "injected bus", debug(skip))]
                    bus: Bus,
                }
            }),
            "debug(skip) requires `#[config(debug)]` on the type"
        );
    }

    #[kithara::test(native, flash(false))]
    fn sdk_field_limit_requires_positive_value_role() {
        let projected = expansion(quote! {
            struct Source {
                #[config(value(u32, self.capacity.load()), sdk)]
                capacity: LiveU32,
            }
        });
        assert!(!projected.contains("sdk"));

        for field in [
            quote!(#[config(value, sdk(max = 0))] capacity: usize),
            quote!(#[config(value, sdk(max = 64), sdk(max = 128))] capacity: usize),
            quote!(#[config(skip = "resource", sdk(max = 64))] capacity: usize),
        ] {
            assert!(
                expand(quote!(struct Source { #field })).is_err(),
                "invalid SDK field declaration was accepted: {field}"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    #[cfg(feature = "patch")]
    fn optional_updates_emit_clear_and_only_declared_defaults_emit_reset() {
        let expanded = expansion(quote! {
            #[config(default, update)]
            struct Settings {
                #[config(value, update, builder(default = Some(3)))]
                width: Option<usize>,
                #[config(value, update, patch(skip))]
                required: usize,
            }
        });

        assert!(expanded.contains("enum SettingsWidthUpdate"));
        assert!(expanded.contains("enum SettingsRequiredUpdate"));
        assert_eq!(
            expanded.matches("Reset").count(),
            2,
            "one variant and one lowering arm"
        );
        assert_eq!(
            expanded.matches("Clear").count(),
            2,
            "one variant and one lowering arm"
        );
    }

    #[kithara::test(native, flash(false))]
    #[cfg(feature = "patch")]
    fn update_rejects_non_value_roles_and_missing_struct_opt_in() {
        assert_eq!(
            refusal(quote! {
                #[config(update)]
                struct Settings {
                    #[config(nested, update)]
                    nested: Nested,
                }
            }),
            "runtime update currently requires a retained value field"
        );
        assert_eq!(
            refusal(quote! {
                struct Settings {
                    #[config(value, update)]
                    value: usize,
                }
            }),
            "field runtime updates require `#[config(update)]` on the struct"
        );
    }
}
