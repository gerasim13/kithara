use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, DeriveInput, Expr, Field, GenericParam, Generics, Ident, Lit, LitStr, Meta, Path,
    Result, Token, Type, meta::ParseNestedMeta, parenthesized, parse::Parser as _,
    punctuated::Punctuated, visit::Visit as _,
};

use super::implementation::{docs, group};

enum Role {
    Value,
    Projection(Box<(Type, Expr)>),
    Nested,
    Skip,
}

/// What one field contributes to its configuration type.
pub(super) struct Member<'a> {
    pub(super) name: &'a Ident,
    pub(super) ty: &'a Type,
    pub(super) construction: Construction,
    pub(super) accessor: Option<TokenStream>,
    pub(super) owner_accessor: Option<TokenStream>,
    /// Whether the generated `Debug` prints the field.
    pub(super) debugged: bool,
    pub(super) retained: Option<Retained>,
}

/// How the builder fills the field.
pub(super) enum Construction {
    /// An argument of the builder's `new`, with its docs and bon options.
    Argument(TokenStream),
    /// `builder(skip)`: no setter; `new` initialises the field itself, because
    /// bon has no `skip` for a function argument.
    Initialised(Expr),
}

pub(super) struct Retained {
    pub(super) declaration: TokenStream,
    pub(super) read: TokenStream,
    pub(super) update: Option<Update>,
}

pub(super) struct Update {
    pub(super) declaration: TokenStream,
    pub(super) field: TokenStream,
    pub(super) lower: TokenStream,
}

/// `wrap(default = expr, with = path)`: the builder takes the projected wire
/// value and wraps it, defaulting to the wrapped `default`.
struct Wrap {
    default: Expr,
    with: Path,
}

/// What a field's `#[config(...)]` attributes declare.
#[derive(Default)]
struct Declaration {
    role: Option<Role>,
    update: bool,
    sdk: bool,
    builder: Option<TokenStream>,
    /// `field(get)` is `Some(false)`, `field(get, copy)` is `Some(true)`.
    accessor: Option<bool>,
    debug_skipped: bool,
    wrap: Option<Wrap>,
}

impl Declaration {
    fn parse(field: &Field) -> Result<Self> {
        let mut declaration = Self::default();
        let mut grouped: Vec<Path> = Vec::new();
        for attr in field
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("config"))
        {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("wrap") {
                    if declaration.wrap.is_some() {
                        return Err(meta.error("duplicate config field wrap"));
                    }
                    declaration.wrap = Some(parse_wrap(&meta.path, group(&meta)?)?);
                } else if meta.path.is_ident("builder")
                    || meta.path.is_ident("field")
                    || meta.path.is_ident("patch")
                    || meta.path.is_ident("debug")
                {
                    if grouped.contains(&meta.path) {
                        return Err(meta.error("duplicate config field attribute group"));
                    }
                    grouped.push(meta.path.clone());
                    declaration.group(&meta.path, group(&meta)?)?;
                } else if meta.path.is_ident("update") {
                    if declaration.update {
                        return Err(meta.error("duplicate config field option"));
                    }
                    declaration.update = true;
                } else if meta.path.is_ident("sdk") {
                    if declaration.sdk {
                        return Err(meta.error("duplicate config field option"));
                    }
                    declaration.sdk = true;
                    parse_sdk(&meta)?;
                } else {
                    if declaration.role.is_some() {
                        return Err(meta.error("select exactly one config field role"));
                    }
                    declaration.role = Some(parse_role(&meta)?);
                }
                Ok(())
            })?;
        }
        Ok(declaration)
    }

    /// Records one `builder(...)`, `field(...)`, `patch(...)` or `debug(...)`
    /// group; `patch(...)` belongs to `Patch`.
    fn group(&mut self, path: &Path, arguments: TokenStream) -> Result<()> {
        if path.is_ident("builder") {
            self.builder = Some(arguments);
        } else if path.is_ident("field") {
            self.accessor = Some(copied(path, arguments)?);
        } else if path.is_ident("debug") {
            let skip: Path = syn::parse2(arguments)?;
            if !skip.is_ident("skip") {
                return Err(syn::Error::new_spanned(skip, "expected debug(skip)"));
            }
            self.debug_skipped = true;
        }
        Ok(())
    }
}

fn parse_sdk(meta: &ParseNestedMeta<'_>) -> Result<()> {
    if !meta.input.peek(syn::token::Paren) {
        return Ok(());
    }
    let content;
    parenthesized!(content in meta.input);
    let maximum: Meta = content.parse()?;
    let Meta::NameValue(maximum) = maximum else {
        return Err(content.error("expected sdk(max = positive integer)"));
    };
    if !maximum.path.is_ident("max")
        || !content.is_empty()
        || !matches!(&maximum.value, Expr::Lit(expr) if matches!(&expr.lit, Lit::Int(value) if value.base10_parse::<u32>().is_ok_and(|value| value > 0)))
    {
        return Err(content.error("expected sdk(max = positive integer)"));
    }
    Ok(())
}

fn parse_role(meta: &ParseNestedMeta<'_>) -> Result<Role> {
    if meta.path.is_ident("value") {
        if !meta.input.peek(syn::token::Paren) {
            return Ok(Role::Value);
        }
        let content;
        parenthesized!(content in meta.input);
        let ty = content.parse()?;
        content.parse::<syn::Token![,]>()?;
        let expression = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("unexpected projection tokens"));
        }
        Ok(Role::Projection(Box::new((ty, expression))))
    } else if meta.path.is_ident("nested") {
        Ok(Role::Nested)
    } else if meta.path.is_ident("skip") {
        let reason: LitStr = meta.value()?.parse()?;
        if reason.value().trim().is_empty() {
            return Err(meta.error("config exclusion requires a reason"));
        }
        Ok(Role::Skip)
    } else {
        Err(meta.error("expected value, value(Type, expression), nested, skip = reason, or update"))
    }
}

pub(super) fn expand<'a>(
    field: &'a Field,
    owner: &DeriveInput,
    snapshot: bool,
    value_default: bool,
) -> Result<Member<'a>> {
    let Declaration {
        role,
        update,
        sdk,
        mut builder,
        accessor,
        debug_skipped,
        wrap,
    } = Declaration::parse(field)?;
    let role = role
        .or_else(|| value_default.then_some(Role::Value))
        .ok_or_else(|| syn::Error::new_spanned(field, "missing config field role"))?;
    validate_role(field, &role, update, sdk, snapshot)?;
    let name = field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(field, "config requires named fields"))?;
    let ty = &field.ty;
    let surface = docs(&field.attrs);
    if let Some(wrap) = wrap {
        if builder.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "wrap replaces the builder group",
            ));
        }
        let Role::Projection(projection) = &role else {
            return Err(syn::Error::new_spanned(
                field,
                "wrap requires value(Type, expression)",
            ));
        };
        let Wrap { default, with } = wrap;
        let wire = &projection.0;
        builder = Some(quote!(default = #with(#default), with = |value: #wire| #with(value)));
    }
    let construction = construction(name, ty, &surface, builder.as_ref())?;
    let owner_accessor = accessor.map(|copy| {
        let output = if copy { quote!(#ty) } else { quote!(&#ty) };
        quote! {
            #(#surface)*
            fn #name(&self) -> #output {
                self.config().#name()
            }
        }
    });
    let accessor = accessor.map(|copy| {
        let (output, body) = if copy {
            (quote!(#ty), quote!(self.#name))
        } else {
            (quote!(&#ty), quote!(&self.#name))
        };
        let visibility = &owner.vis;
        quote! {
            #(#surface)*
            #visibility fn #name(&self) -> #output {
                #body
            }
        }
    });
    let retained = if snapshot {
        retained(field, owner, name, role, update, builder.as_ref())?
    } else {
        None
    };
    Ok(Member {
        name,
        ty,
        construction,
        accessor,
        owner_accessor,
        debugged: !debug_skipped,
        retained,
    })
}

fn parse_wrap(group: &Path, arguments: TokenStream) -> Result<Wrap> {
    let mut default = None;
    let mut with = None;
    syn::meta::parser(|meta| {
        if meta.path.is_ident("default") {
            if default.replace(meta.value()?.parse()?).is_some() {
                return Err(meta.error("duplicate wrap default"));
            }
        } else if meta.path.is_ident("with") {
            if with.replace(meta.value()?.parse()?).is_some() {
                return Err(meta.error("duplicate wrap constructor"));
            }
        } else {
            return Err(meta.error("expected default = expression or with = path"));
        }
        Ok(())
    })
    .parse2(arguments)?;
    let (Some(default), Some(with)) = (default, with) else {
        return Err(syn::Error::new_spanned(
            group,
            "wrap requires default = expression and with = constructor",
        ));
    };
    Ok(Wrap { default, with })
}

/// Reads a `builder(...)` group: `skip` or `skip = expr` keeps the field out
/// of the builder, anything else is bon's option list for the argument.
fn construction(
    name: &Ident,
    ty: &Type,
    surface: &[&Attribute],
    builder: Option<&TokenStream>,
) -> Result<Construction> {
    let Some(builder) = builder else {
        return Ok(Construction::Argument(quote! { #(#surface)* #name: #ty }));
    };
    let options = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(builder.clone())?;
    let Some(skip) = options.iter().find(|option| option.path().is_ident("skip")) else {
        return Ok(Construction::Argument(
            quote! { #(#surface)* #[builder(#builder)] #name: #ty },
        ));
    };
    if options.len() > 1 {
        return Err(syn::Error::new_spanned(
            skip,
            "builder(skip) takes no other builder options",
        ));
    }
    Ok(Construction::Initialised(match skip {
        Meta::Path(_) => syn::parse_quote!(::core::default::Default::default()),
        Meta::NameValue(skip) => skip.value.clone(),
        Meta::List(_) => {
            return Err(syn::Error::new_spanned(
                skip,
                "expected builder(skip) or builder(skip = expression)",
            ));
        }
    }))
}

/// Reads a `field(...)` group: `get` returns a reference, `get, copy` a copy.
fn copied(group: &Path, arguments: TokenStream) -> Result<bool> {
    let mut get = false;
    let mut copy = false;
    for option in Punctuated::<Meta, Token![,]>::parse_terminated.parse2(arguments)? {
        let flag = match &option {
            Meta::Path(path) if path.is_ident("get") => &mut get,
            Meta::Path(path) if path.is_ident("copy") => &mut copy,
            _ => {
                return Err(syn::Error::new_spanned(
                    option,
                    "a config accessor is field(get) or field(get, copy)",
                ));
            }
        };
        if *flag {
            return Err(syn::Error::new_spanned(option, "duplicate accessor option"));
        }
        *flag = true;
    }
    if !get {
        return Err(syn::Error::new_spanned(
            group,
            "a config accessor is field(get) or field(get, copy)",
        ));
    }
    Ok(copy)
}

fn retained(
    field: &Field,
    owner: &DeriveInput,
    name: &Ident,
    role: Role,
    update: bool,
    builder: Option<&TokenStream>,
) -> Result<Option<Retained>> {
    let original_type = &field.ty;
    let (ty, expression): (Type, Expr) = match role {
        Role::Skip => return Ok(None),
        Role::Value => (
            original_type.clone(),
            syn::parse_quote!(::core::clone::Clone::clone(&self.#name)),
        ),
        Role::Nested => (
            syn::parse_quote!(<#original_type as ::kithara_config::Config>::Values),
            syn::parse_quote!(::kithara_config::Config::values(&self.#name)),
        ),
        Role::Projection(projection) => *projection,
    };
    if names_a_parameter(&ty, &owner.generics) {
        return Err(syn::Error::new_spanned(
            ty,
            "snapshot types cannot depend on resource generics; use value(OwnedType, expression)",
        ));
    }
    let surface = docs(&field.attrs);
    let update = update
        .then(|| update_tokens(builder, &owner.ident, name, original_type, &surface))
        .transpose()?;
    Ok(Some(Retained {
        declaration: quote! { #(#surface)* pub #name: #ty },
        read: quote! { #name: #expression },
        update,
    }))
}

fn validate_role(
    field: &Field,
    role: &Role,
    update: bool,
    sdk: bool,
    snapshot: bool,
) -> Result<()> {
    if sdk && !matches!(role, Role::Value | Role::Projection(_)) {
        return Err(syn::Error::new_spanned(
            field,
            "SDK exposure requires a value or projected value field",
        ));
    }
    if update && !matches!(role, Role::Value) {
        return Err(syn::Error::new_spanned(
            field,
            "runtime update currently requires a retained value field",
        ));
    }
    if update && !snapshot {
        return Err(syn::Error::new_spanned(
            field,
            "construction inputs cannot declare retained runtime updates",
        ));
    }
    if !snapshot && matches!(role, Role::Projection(_)) {
        return Err(syn::Error::new_spanned(
            field,
            "construction inputs do not produce projected values",
        ));
    }
    Ok(())
}

fn update_tokens(
    builder: Option<&TokenStream>,
    owner: &Ident,
    name: &Ident,
    ty: &Type,
    surface: &[&Attribute],
) -> Result<Update> {
    let enum_name = format_ident!("{}{}Update", owner, upper_camel(name));
    let optional = option_inner(ty);
    let payload = optional.unwrap_or(ty);
    let default = builder.map(builder_default).transpose()?.flatten();
    let clear = optional.map(|_| quote! { Clear, });
    let reset = default.as_ref().map(|_| quote! { Reset, });
    let set = if optional.is_some() {
        quote! { target.#name = ::core::option::Option::Some(value); }
    } else {
        quote! { target.#name = value; }
    };
    let clear_lower = optional.map(|_| {
        quote! { #enum_name::Clear => { target.#name = ::core::option::Option::None; } }
    });
    let reset_lower = default.map(|default| {
        quote! { #enum_name::Reset => { target.#name = #default; } }
    });
    Ok(Update {
        declaration: quote! {
            #(#surface)*
            #[derive(::core::default::Default)]
            #[non_exhaustive]
            pub enum #enum_name {
                #[default]
                Unchanged,
                Set { value: #payload },
                #clear
                #reset
            }
        },
        field: quote! { #(#surface)* pub #name: #enum_name },
        lower: quote! {
            match update.#name {
                #enum_name::Unchanged => {}
                #enum_name::Set { value } => { #set }
                #clear_lower
                #reset_lower
            }
        },
    })
}

/// The value `builder(default)` or `builder(default = expr)` fills in, which
/// is what a runtime `Reset` restores.
fn builder_default(builder: &TokenStream) -> Result<Option<Expr>> {
    let mut default = None;
    for option in Punctuated::<Meta, Token![,]>::parse_terminated.parse2(builder.clone())? {
        if !option.path().is_ident("default") {
            continue;
        }
        if default.is_some() {
            return Err(syn::Error::new_spanned(option, "duplicate builder default"));
        }
        default = Some(match option {
            Meta::NameValue(option) => option.value,
            _ => syn::parse_quote!(::core::default::Default::default()),
        });
    }
    Ok(default)
}

fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    match arguments.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn upper_camel(ident: &Ident) -> String {
    ident
        .to_string()
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(char::to_uppercase)
                .into_iter()
                .flatten()
                .chain(chars)
                .collect::<String>()
        })
        .collect()
}

/// Whether `ty` names one of the type's generic parameters.
pub(super) fn names_a_parameter(ty: &Type, generics: &Generics) -> bool {
    let mut usage = GenericUse {
        generics,
        found: false,
    };
    usage.visit_type(ty);
    usage.found
}

struct GenericUse<'a> {
    generics: &'a Generics,
    found: bool,
}

impl<'ast> syn::visit::Visit<'ast> for GenericUse<'_> {
    fn visit_ident(&mut self, ident: &'ast Ident) {
        self.found |= self
            .generics
            .params
            .iter()
            .any(|parameter| match parameter {
                GenericParam::Type(parameter) => parameter.ident == *ident,
                GenericParam::Const(parameter) => parameter.ident == *ident,
                GenericParam::Lifetime(parameter) => parameter.lifetime.ident == *ident,
            });
    }
}
