use std::{cmp::Ordering, ops::Range};

use anyhow::Result;
use proc_macro2::TokenTree;
use syn::{
    Attribute, Field, Fields, Item, Meta, Path, Token, Type, Visibility, parse::Parser,
    punctuated::Punctuated, spanned::Spanned,
};

use super::{
    super::{Check, Context},
    safety::{self, OrderSafety},
};
use crate::{
    common::{
        fix::{ExpansionError, FixOutcome, SourceRewriter, expand_blocks},
        violation::Violation,
        walker::relative_to,
    },
    style::config::StructFieldOrderConfig,
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "struct_field_order";
}

pub(crate) struct StructFieldOrder;

impl Check for StructFieldOrder {
    fn fix(&self, ctx: &Context<'_>) -> Result<FixOutcome> {
        let cfg = &ctx.config.thresholds.struct_field_order;
        let closed_root = safety::closed_workspace_root(ctx);
        let mut outcome = FixOutcome::default();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let Some(src) = ctx.scan.source(path) else {
                continue;
            };
            let Ok(file) = syn::parse_file(&src) else {
                continue;
            };
            let rel = relative_to(ctx.workspace_root, path)
                .to_string_lossy()
                .replace('\\', "/");
            let mut rw = SourceRewriter::new(&src);
            fix_items(
                cfg,
                &rel,
                &src,
                &file.items,
                OrderSafety::new(closed_root.as_deref() == Some(path.as_path()), &file.attrs),
                &mut rw,
                &mut outcome.skipped,
            );
            if !rw.is_empty() {
                let new_src = rw.finish()?;
                ctx.scan.write(path, new_src)?;
                outcome.writes += 1;
            }
        }
        Ok(outcome)
    }

    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.struct_field_order;
        let closed_root = safety::closed_workspace_root(ctx);
        let mut violations = Vec::new();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let Ok(file) = ctx.scan.parse_file(path) else {
                continue;
            };
            let rel = relative_to(ctx.workspace_root, path)
                .to_string_lossy()
                .replace('\\', "/");
            scan_items(
                cfg,
                &rel,
                &file.items,
                OrderSafety::new(closed_root.as_deref() == Some(path.as_path()), &file.attrs),
                &mut Vec::new(),
                &mut violations,
            );
        }
        violations.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(violations)
    }
}

pub(super) fn fix_items<'src>(
    cfg: &StructFieldOrderConfig,
    rel: &str,
    src: &'src str,
    items: &[Item],
    safety: OrderSafety,
    rw: &mut SourceRewriter<'src>,
    skipped: &mut Vec<String>,
) {
    let safety = safety.items(items);
    for item in items {
        match item {
            Item::Struct(s) => {
                if is_exempt(&s.attrs, cfg) {
                    continue;
                }
                if let Fields::Named(named) = &s.fields
                    && let Err(reason) = fix_field_block(
                        cfg,
                        src,
                        named.brace_token.span.open().byte_range().end
                            ..named.brace_token.span.close().byte_range().start,
                        &named.named,
                        safety.attributes(&s.attrs).generics(&s.generics),
                        rw,
                    )
                {
                    skipped.push(format!(
                        "{rel}:{}: struct `{}`: {reason}",
                        s.ident.span().start().line,
                        s.ident
                    ));
                }
            }
            Item::Union(u) => {
                if is_exempt(&u.attrs, cfg) {
                    continue;
                }
                if let Err(reason) = fix_field_block(
                    cfg,
                    src,
                    u.fields.brace_token.span.open().byte_range().end
                        ..u.fields.brace_token.span.close().byte_range().start,
                    &u.fields.named,
                    safety.attributes(&u.attrs).generics(&u.generics),
                    rw,
                ) {
                    skipped.push(format!(
                        "{rel}:{}: union `{}`: {reason}",
                        u.ident.span().start().line,
                        u.ident
                    ));
                }
            }
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    fix_items(
                        cfg,
                        rel,
                        src,
                        inner,
                        safety.attributes(&m.attrs),
                        rw,
                        skipped,
                    );
                }
            }
            _ => {}
        }
    }
}

fn fix_field_block<'src>(
    cfg: &StructFieldOrderConfig,
    src: &'src str,
    scope_bytes: Range<usize>,
    named: &Punctuated<Field, Token![,]>,
    safety: OrderSafety,
    rw: &mut SourceRewriter<'src>,
) -> Result<(), String> {
    let fields: Vec<&Field> = named.iter().collect();
    if fields.len() < 2 {
        return Ok(());
    }

    if has_heterogeneous_cfg(&fields) {
        return Err("fields have heterogeneous `#[cfg(...)]` attributes".to_string());
    }

    let actual = field_keys(cfg, &fields);
    let mut expected = actual.clone();
    expected.sort_by(cmp_field_key);
    if actual
        .iter()
        .map(|k| k.idx)
        .eq(expected.iter().map(|k| k.idx))
    {
        return Ok(());
    }

    safety.permutation(
        &fields,
        &expected.iter().map(|field| field.idx).collect::<Vec<_>>(),
    )?;

    if !named.trailing_punct() {
        return Err("last field has no trailing comma".to_string());
    }

    let item_spans: Vec<Range<usize>> = named
        .pairs()
        .filter_map(|pair| {
            pair.punct()
                .map(|comma| pair.value().span().byte_range().start..comma.span().byte_range().end)
        })
        .collect();
    let blocks = match expand_blocks(src, scope_bytes, &item_spans) {
        Ok(b) => b,
        Err(ExpansionError::FloatingComment { line, snippet }) => {
            return Err(format!("floating comment at line {line}: `{snippet}`"));
        }
        Err(other) => return Err(format!("engine error: {other:?}")),
    };

    let texts: Vec<String> = blocks
        .iter()
        .map(|b| src[b.bytes.clone()].to_string())
        .collect();
    for (slot_idx, expected_key) in expected.iter().enumerate() {
        let source_idx = expected_key.idx;
        if source_idx == slot_idx {
            continue;
        }
        rw.replace(blocks[slot_idx].bytes.clone(), texts[source_idx].clone());
    }
    Ok(())
}

/// Returns true if any field carries a `#[cfg(...)]` attribute and its
/// neighbour does not, OR any two fields carry different `#[cfg]` text.
/// Conservative: any cfg presence among the fields trips the check.
fn has_heterogeneous_cfg(fields: &[&Field]) -> bool {
    let cfgs: Vec<String> = fields.iter().map(|f| cfg_signature(&f.attrs)).collect();
    cfgs.iter().any(|c| !c.is_empty()) && cfgs.windows(2).any(|w| w[0] != w[1])
}

fn cfg_signature(attrs: &[Attribute]) -> String {
    let mut sigs: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .map(|a| match &a.meta {
            Meta::List(list) => list.tokens.to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    sigs.sort();
    sigs.join("|")
}

pub(super) fn scan_items(
    cfg: &StructFieldOrderConfig,
    rel: &str,
    items: &[Item],
    safety: OrderSafety,
    mod_path: &mut Vec<String>,
    out: &mut Vec<Violation>,
) {
    let safety = safety.items(items);
    for item in items {
        match item {
            Item::Struct(s) => {
                if is_exempt(&s.attrs, cfg) {
                    continue;
                }
                if let Fields::Named(named) = &s.fields {
                    let collected: Vec<&Field> = named.named.iter().collect();
                    check_field_block(
                        cfg,
                        rel,
                        mod_path,
                        &s.ident.to_string(),
                        &collected,
                        safety.attributes(&s.attrs).generics(&s.generics),
                        out,
                    );
                }
            }
            Item::Union(u) => {
                if is_exempt(&u.attrs, cfg) {
                    continue;
                }
                let collected: Vec<&Field> = u.fields.named.iter().collect();
                check_field_block(
                    cfg,
                    rel,
                    mod_path,
                    &u.ident.to_string(),
                    &collected,
                    safety.attributes(&u.attrs).generics(&u.generics),
                    out,
                );
            }
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    mod_path.push(m.ident.to_string());
                    scan_items(cfg, rel, inner, safety.attributes(&m.attrs), mod_path, out);
                    mod_path.pop();
                }
            }
            _ => {}
        }
    }
}

/// Mirrors the autofix safety model: reordering across heterogeneous `#[cfg(...)]` field attributes
/// changes which fields compile together, so detection refuses these the same way the fix does.
fn check_field_block(
    cfg: &StructFieldOrderConfig,
    rel: &str,
    mod_path: &[String],
    type_name: &str,
    fields: &[&Field],
    safety: OrderSafety,
    out: &mut Vec<Violation>,
) {
    if fields.len() < 2 {
        return;
    }
    if has_heterogeneous_cfg(fields) {
        return;
    }
    let actual = field_keys(cfg, fields);

    let mut expected = actual.clone();
    expected.sort_by(cmp_field_key);

    if actual
        .iter()
        .map(|k| k.idx)
        .eq(expected.iter().map(|k| k.idx))
    {
        return;
    }

    let mod_prefix = if mod_path.is_empty() {
        String::new()
    } else {
        format!("{}::", mod_path.join("::"))
    };
    let key = format!("{rel}::{mod_prefix}{type_name}");
    let actual_summary = actual
        .iter()
        .map(|k| k.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let expected_summary = expected
        .iter()
        .map(|k| k.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let refusal = safety
        .permutation(
            fields,
            &expected.iter().map(|field| field.idx).collect::<Vec<_>>(),
        )
        .err();
    let msg = refusal.map_or_else(
        || {
            format!(
                "declaration `{type_name}` field order should be (visibility, type, name): \
                 expected [{expected_summary}], found [{actual_summary}]"
            )
        },
        |reason| {
            format!(
                "declaration `{type_name}` field order candidate (visibility, type, name): \
                 canonical [{expected_summary}], found [{actual_summary}]; autofix refused: {reason}"
            )
        },
    );
    out.push(Violation::warn(consts::ID, key, msg));
}

fn field_keys(cfg: &StructFieldOrderConfig, fields: &[&Field]) -> Vec<FieldKey> {
    let order = build_visibility_order(&cfg.visibility_order);
    fields
        .iter()
        .enumerate()
        .map(|(idx, field)| FieldKey {
            idx,
            builder_bucket: builder_bucket(&field.attrs),
            vis_bucket: vis_bucket(&order, &field.vis),
            type_key: type_sort_key(&field.ty),
            name: field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        })
        .collect()
}

#[derive(Debug, Clone)]
struct FieldKey {
    builder_bucket: BuilderRole,
    name: String,
    type_key: String,
    idx: usize,
    vis_bucket: usize,
}

fn cmp_field_key(a: &FieldKey, b: &FieldKey) -> Ordering {
    a.builder_bucket.cmp(&b.builder_bucket).then_with(|| {
        if a.builder_bucket.is_positional() {
            a.idx.cmp(&b.idx)
        } else {
            a.vis_bucket
                .cmp(&b.vis_bucket)
                .then_with(|| a.type_key.cmp(&b.type_key))
                .then_with(|| a.name.cmp(&b.name))
        }
    })
}

/// What a `bon` builder makes of a field, in the order the roles appear in the
/// generated API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BuilderRole {
    /// An argument of the builder's starting function.
    StartFn,
    /// A builder-private field, set by neither caller nor setter.
    Field,
    /// An argument of the builder's finishing function.
    FinishFn,
    /// A field the builder gives a setter of its own.
    Setter,
}

impl BuilderRole {
    /// Whether the role puts the field in a function signature, where the
    /// declaration order is the call order and sorting would change the API.
    const fn is_positional(self) -> bool {
        matches!(self, Self::StartFn | Self::FinishFn)
    }
}

fn builder_bucket(attrs: &[Attribute]) -> BuilderRole {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("builder"))
        .filter_map(|attr| match &attr.meta {
            Meta::List(list) => list.tokens.clone().into_iter().find_map(|token| {
                let TokenTree::Ident(ident) = token else {
                    return None;
                };
                match ident.to_string().as_str() {
                    "start_fn" => Some(BuilderRole::StartFn),
                    "field" => Some(BuilderRole::Field),
                    "finish_fn" => Some(BuilderRole::FinishFn),
                    _ => None,
                }
            }),
            _ => None,
        })
        .min()
        .unwrap_or(BuilderRole::Setter)
}

/// Map each visibility token in config to its bucket index.
fn build_visibility_order(order: &[String]) -> Vec<&str> {
    order.iter().map(String::as_str).collect()
}

fn vis_bucket(order: &[&str], vis: &Visibility) -> usize {
    let token = vis_token(vis);
    order
        .iter()
        .position(|t| *t == token)
        .unwrap_or(order.len())
}

fn vis_token(vis: &Visibility) -> &'static str {
    match vis {
        Visibility::Public(_) => "pub",
        Visibility::Restricted(r) => {
            if r.path.is_ident("crate") {
                "pub(crate)"
            } else if r.path.is_ident("super") {
                "pub(super)"
            } else {
                "pub(in)"
            }
        }
        Visibility::Inherited => "private",
    }
}

fn is_exempt(attrs: &[Attribute], cfg: &StructFieldOrderConfig) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .get_ident()
            .is_some_and(|id| cfg.exempt_attrs.iter().any(|n| id == n))
            || derived_paths(a)
                .iter()
                .any(|path| cfg.exempt_derives.contains(path))
    })
}

/// The `::`-joined paths an attribute derives, directly or through `cfg_attr`.
fn derived_paths(attr: &Attribute) -> Vec<String> {
    let Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    if list.path.is_ident("derive") {
        return parse_derive_list(list.tokens.clone());
    }
    if !list.path.is_ident("cfg_attr") {
        return Vec::new();
    }
    let tokens: Vec<TokenTree> = list.tokens.clone().into_iter().collect();
    tokens
        .windows(2)
        .filter_map(|pair| match pair {
            [TokenTree::Ident(id), TokenTree::Group(group)] if id == "derive" => {
                Some(parse_derive_list(group.stream()))
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn parse_derive_list(tokens: proc_macro2::TokenStream) -> Vec<String> {
    Punctuated::<Path, Token![,]>::parse_terminated
        .parse2(tokens)
        .map(|paths| {
            paths
                .iter()
                .map(|path| {
                    path.segments
                        .iter()
                        .map(|segment| segment.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Stable, last-segment-based sort key for a `Type`.
/// Generics, refs, and pointers are folded into the head ident so
/// `Vec<Foo>` and `Vec<Bar>` group together and then sort by field name.
fn type_sort_key(ty: &Type) -> String {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        Type::Reference(r) => format!("&{}", type_sort_key(&r.elem)),
        Type::Array(a) => format!("[{}]", type_sort_key(&a.elem)),
        Type::Slice(s) => format!("[{}]", type_sort_key(&s.elem)),
        Type::Tuple(t) => {
            let parts = t.elems.iter().map(type_sort_key).collect::<Vec<_>>();
            format!("({})", parts.join(","))
        }
        Type::Ptr(p) => format!("*{}", type_sort_key(&p.elem)),
        Type::Paren(p) => type_sort_key(&p.elem),
        Type::Group(g) => type_sort_key(&g.elem),
        Type::Never(_) => "!".to_string(),
        Type::TraitObject(_) => "dyn".to_string(),
        Type::ImplTrait(_) => "impl".to_string(),
        Type::FnPtr(_) => "fn".to_string(),
        Type::Macro(_) => "macro".to_string(),
        Type::Verbatim(_) => "?".to_string(),
        _ => "_".to_string(),
    }
}
