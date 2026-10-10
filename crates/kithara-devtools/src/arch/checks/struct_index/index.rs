use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use glob::Pattern;
use syn::{Block, Fields, ImplItem, Item, ItemImpl, Pat, Signature, Stmt, Visibility};

use super::{assignments::collect_assigned_fields, literals::LiteralVisitor};
use crate::{
    arch::checks::{
        Context,
        declaration_index::{DeclarationIndex, DeclarationKey, known_attrs},
    },
    common::{
        suppress::Suppressions,
        walker::{matches_any, relative_to, workspace_rs_files_scoped},
    },
};

#[derive(Debug)]
pub(crate) struct StructInfo {
    pub(crate) name: String,
    pub(crate) rel: String,
    pub(crate) field_names: Vec<String>,
    pub(crate) is_pub: bool,
    pub(crate) line: usize,
}

#[derive(Debug)]
pub(crate) struct LiteralSite {
    pub(crate) field_exprs: BTreeMap<String, String>,
    pub(crate) parent_fn: Option<DeclarationKey>,
    pub(crate) has_rest: bool,
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceStructIndex {
    pub(crate) destructuring_consumers: BTreeMap<DeclarationKey, BTreeSet<DeclarationKey>>,
    pub(crate) impl_method_counts: BTreeMap<DeclarationKey, usize>,
    pub(crate) literals: BTreeMap<DeclarationKey, Vec<LiteralSite>>,
    pub(crate) structs: BTreeMap<DeclarationKey, StructInfo>,
    pub(crate) assigned_fields: HashMap<String, HashSet<String>>,
    pub(crate) suppressions: HashMap<String, Suppressions>,
}

pub(super) struct SourceScope<'a> {
    pub(super) declarations: &'a DeclarationIndex,
    pub(super) rel: &'a str,
    pub(super) inline: &'a [String],
}

pub(crate) fn build_index(
    ctx: &Context<'_>,
    exempt_globs: &[Pattern],
) -> Result<WorkspaceStructIndex> {
    let declarations = ctx.declaration_index()?;
    let mut idx = WorkspaceStructIndex::default();
    for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
        let rel = relative_to(ctx.workspace_root, &path);
        if matches_any(exempt_globs, rel) {
            continue;
        }
        let Some((src, file)) = ctx.parsed_source(&path)? else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        idx.suppressions
            .insert(rel.clone(), Suppressions::parse(src));
        collect_assigned_fields(file, &rel, &mut idx);
        Collector {
            declarations,
            rel: &rel,
            inline: Vec::new(),
            idx: &mut idx,
        }
        .items(&file.items);
    }
    Ok(idx)
}

struct Collector<'a> {
    declarations: &'a DeclarationIndex,
    rel: &'a str,
    inline: Vec<String>,
    idx: &'a mut WorkspaceStructIndex,
}

impl Collector<'_> {
    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Mod(module) => {
                    if let Some((_, inner)) = &module.content {
                        self.inline.push(module.ident.to_string());
                        self.items(inner);
                        self.inline.pop();
                    }
                }
                Item::Struct(item) => {
                    let Fields::Named(fields) = &item.fields else {
                        continue;
                    };
                    let name = item.ident.to_string();
                    let Some(key) = self
                        .declarations
                        .key_for_decl(self.rel, &self.inline, &name)
                    else {
                        continue;
                    };
                    self.idx.structs.insert(
                        key,
                        StructInfo {
                            name,
                            rel: self.rel.to_string(),
                            line: item.ident.span().start().line,
                            field_names: fields
                                .named
                                .iter()
                                .filter_map(|field| field.ident.as_ref().map(ToString::to_string))
                                .collect(),
                            is_pub: matches!(item.vis, Visibility::Public(_)),
                        },
                    );
                }
                Item::Impl(item) => self.inspect_impl(item),
                Item::Fn(item) if known_attrs(&item.attrs) => {
                    let function = self.declarations.key_for_decl(
                        self.rel,
                        &self.inline,
                        &item.sig.ident.to_string(),
                    );
                    self.inspect_fn(
                        &item.sig,
                        &item.block,
                        FunctionScope {
                            owner: None,
                            function,
                            inherited: None,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    fn inspect_impl(&mut self, item: &ItemImpl) {
        if !known_attrs(&item.attrs) {
            return;
        }
        let owner = self.declarations.resolve_impl(self.rel, &self.inline, item);
        if item.trait_.is_none()
            && let Some(key) = owner.as_ref()
        {
            *self.idx.impl_method_counts.entry(key.clone()).or_default() += item
                .items
                .iter()
                .filter(|item| matches!(item, ImplItem::Fn(_)))
                .count();
        }
        for method in &item.items {
            if let ImplItem::Fn(method) = method
                && known_attrs(&method.attrs)
            {
                self.inspect_fn(
                    &method.sig,
                    &method.block,
                    FunctionScope {
                        owner: owner.as_ref(),
                        function: None,
                        inherited: Some(&item.generics),
                    },
                );
            }
        }
    }

    fn inspect_fn(&mut self, signature: &Signature, block: &Block, function: FunctionScope<'_>) {
        let scope = SourceScope {
            declarations: self.declarations,
            rel: self.rel,
            inline: &self.inline,
        };
        let mut visitor = LiteralVisitor::new(scope, self.idx, function.owner);
        visitor.bindings(signature, function.inherited, block);
        if let Some(function) = function.function
            && let Some(Stmt::Local(local)) = block.stmts.first()
            && let Some(path) = leading_destructure_path(&local.pat)
            && let Some(key) = visitor.resolve_path(path)
        {
            visitor
                .idx
                .destructuring_consumers
                .entry(key)
                .or_default()
                .insert(function);
        }
        syn::visit::Visit::visit_block(&mut visitor, block);
    }
}

struct FunctionScope<'a> {
    owner: Option<&'a DeclarationKey>,
    function: Option<DeclarationKey>,
    inherited: Option<&'a syn::Generics>,
}

fn leading_destructure_path(pat: &Pat) -> Option<&syn::Path> {
    match pat {
        Pat::Struct(pat) if pat.qself.is_none() => Some(&pat.path),
        Pat::Reference(pat) => leading_destructure_path(&pat.pat),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn build_index_from_source(src: &str) -> WorkspaceStructIndex {
    const REL: &str = "crates/fixture/src/lib.rs";
    let dir = tempfile::tempdir().expect("temporary workspace");
    let source = dir.path().join(REL);
    std::fs::create_dir_all(source.parent().expect("source directory"))
        .expect("create source directory");
    std::fs::write(&source, src).expect("fixture source");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/fixture\"]\nresolver = \"2\"\n",
    )
    .expect("workspace manifest");
    std::fs::write(
        dir.path().join("crates/fixture/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("package manifest");
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(dir.path().join("Cargo.toml"))
        .no_deps()
        .exec()
        .expect("fixture metadata");
    let config = crate::arch::config::ArchConfig::default();
    let scope = crate::common::scope::Scope::default();
    build_index(&Context::new(&config, &metadata, dir.path(), &scope), &[])
        .expect("workspace struct index")
}
