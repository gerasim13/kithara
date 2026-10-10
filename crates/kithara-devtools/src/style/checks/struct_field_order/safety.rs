use std::path::PathBuf;

use syn::{
    Attribute, Field, GenericParam, Generics, Item, ItemMod, Macro, Type, UseTree,
    visit::{Visit, visit_item, visit_item_mod},
};

use super::super::Context;
use crate::common::exclude::item_attrs;

#[derive(Clone, Copy)]
pub(super) struct OrderSafety {
    closed_lineage: bool,
    primitives_known: bool,
    transparent_ancestors: bool,
}

impl OrderSafety {
    pub(super) fn new(closed_lineage: bool, attrs: &[Attribute]) -> Self {
        Self {
            closed_lineage,
            primitives_known: true,
            transparent_ancestors: !opaque_attributes(attrs),
        }
    }

    pub(super) fn attributes(mut self, attrs: &[Attribute]) -> Self {
        self.transparent_ancestors &= !opaque_attributes(attrs);
        self
    }

    pub(super) fn generics(mut self, generics: &Generics) -> Self {
        self.primitives_known &= generics_preserve_primitives(generics);
        self
    }

    pub(super) fn items(mut self, items: &[Item]) -> Self {
        self.primitives_known &= primitive_names_known(items);
        self
    }

    pub(super) fn permutation(self, fields: &[&Field], expected: &[usize]) -> Result<(), String> {
        if !self.transparent_ancestors || fields.iter().any(|field| opaque_attributes(&field.attrs))
        {
            return Err("opaque or order-sensitive attributes/derives".to_string());
        }
        let dropping: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, field)| !drop_inert(&field.ty, self.primitives_known))
            .map(|(index, _)| index)
            .collect();
        if !dropping.iter().copied().eq(expected
            .iter()
            .copied()
            .filter(|&index| !drop_inert(&fields[index].ty, self.primitives_known)))
        {
            return Err("relative drop order is not proved unchanged".to_string());
        }
        if let Some(last) = fields.last()
            && !sized(&last.ty, self.primitives_known)
            && expected.last().copied() != Some(fields.len() - 1)
        {
            return Err("the final field may be unsized and must remain last".to_string());
        }
        if !self.closed_lineage {
            return Err(
                "source lineage is unresolved; only a closed single-target source can be reordered"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// shortcut: only a single Cargo source with no source-generating syntax is closed;
/// broader coverage needs one shared owner proving every module/include context.
pub(super) fn closed_workspace_root(ctx: &Context<'_>) -> Option<PathBuf> {
    let mut targets = ctx
        .metadata
        .packages
        .iter()
        .filter(|package| ctx.metadata.workspace_members.contains(&package.id))
        .flat_map(|package| &package.targets);
    let target = targets.next()?;
    if targets.next().is_some() {
        return None;
    }
    let path = target.src_path.as_std_path();
    let file = ctx.scan.parse_file(path).ok()?;
    closed_file(&file).then(|| path.to_path_buf())
}

pub(super) fn closed_file(file: &syn::File) -> bool {
    let mut proof = ClosedSource(true);
    proof.visit_file(file);
    proof.0
}

struct ClosedSource(bool);

impl<'ast> Visit<'ast> for ClosedSource {
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        self.0 &= !opaque_attributes(std::slice::from_ref(attr));
    }

    fn visit_item(&mut self, item: &'ast Item) {
        self.0 &= !matches!(item, Item::Verbatim(_));
        visit_item(self, item);
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        self.0 &= item.content.is_some();
        visit_item_mod(self, item);
    }

    fn visit_macro(&mut self, _item: &'ast Macro) {
        self.0 = false;
    }
}

pub(super) fn opaque_attributes(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        !attr.path().get_ident().is_some_and(|name| {
            matches!(
                name.to_string().as_str(),
                "cfg"
                    | "doc"
                    | "deprecated"
                    | "must_use"
                    | "non_exhaustive"
                    | "feature"
                    | "forbid"
                    | "deny"
                    | "warn"
                    | "allow"
                    | "expect"
            )
        })
    })
}

fn primitive_names_known(items: &[Item]) -> bool {
    items.iter().all(|item| {
        if opaque_attributes(item_attrs(item)) {
            return false;
        }
        match item {
            Item::Struct(item) => !primitive_name(&item.ident.to_string()),
            Item::Enum(item) => !primitive_name(&item.ident.to_string()),
            Item::Union(item) => !primitive_name(&item.ident.to_string()),
            Item::Type(item) => !primitive_name(&item.ident.to_string()),
            Item::Trait(item) => !primitive_name(&item.ident.to_string()),
            Item::TraitAlias(item) => !primitive_name(&item.ident.to_string()),
            Item::Mod(item) => !primitive_name(&item.ident.to_string()),
            Item::ExternCrate(item) => !primitive_name(
                &item
                    .rename
                    .as_ref()
                    .map_or(&item.ident, |(_, name)| name)
                    .to_string(),
            ),
            Item::Use(item) => !use_may_shadow_primitive(&item.tree),
            Item::Macro(_) | Item::ForeignMod(_) | Item::Verbatim(_) => false,
            _ => true,
        }
    })
}

fn generics_preserve_primitives(generics: &Generics) -> bool {
    !generics.params.iter().any(|param| {
        matches!(param, GenericParam::Type(param) if primitive_name(&param.ident.to_string()))
    })
}

fn use_may_shadow_primitive(tree: &UseTree) -> bool {
    match tree {
        UseTree::Path(path) => {
            primitive_name(&path.ident.to_string()) || use_may_shadow_primitive(&path.tree)
        }
        UseTree::Name(name) => primitive_name(&name.ident.to_string()),
        UseTree::Rename(rename) => primitive_name(&rename.rename.to_string()),
        UseTree::Glob(_) => true,
        UseTree::Group(group) => group.items.iter().any(use_may_shadow_primitive),
    }
}

fn primitive_name(name: &str) -> bool {
    matches!(
        name.strip_prefix("r#").unwrap_or(name),
        "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f16"
            | "f32"
            | "f64"
            | "f128"
    )
}

fn primitive_type(ty: &Type, names_known: bool) -> bool {
    names_known
        && matches!(ty, Type::Path(path) if path.qself.is_none()
            && path.path.leading_colon.is_none()
            && path.path.segments.len() == 1
            && path.path.segments[0].arguments.is_empty()
            && !path.path.segments[0].ident.to_string().starts_with("r#")
            && primitive_name(&path.path.segments[0].ident.to_string()))
}

fn drop_inert(ty: &Type, names_known: bool) -> bool {
    match ty {
        Type::Reference(_) | Type::Ptr(_) | Type::FnPtr(_) | Type::Never(_) => true,
        Type::Array(array) => drop_inert(&array.elem, names_known),
        Type::Tuple(tuple) => tuple.elems.iter().all(|ty| drop_inert(ty, names_known)),
        Type::Paren(inner) => drop_inert(&inner.elem, names_known),
        Type::Group(inner) => drop_inert(&inner.elem, names_known),
        _ => primitive_type(ty, names_known),
    }
}

fn sized(ty: &Type, names_known: bool) -> bool {
    match ty {
        Type::Reference(_) | Type::Ptr(_) | Type::FnPtr(_) | Type::Never(_) | Type::Array(_) => {
            true
        }
        Type::Tuple(tuple) => tuple.elems.iter().all(|ty| sized(ty, names_known)),
        Type::Paren(inner) => sized(&inner.elem, names_known),
        Type::Group(inner) => sized(&inner.elem, names_known),
        _ => {
            primitive_type(ty, names_known)
                && !matches!(ty, Type::Path(path) if path.path.is_ident("str"))
        }
    }
}
