use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use syn::Item;

use super::{
    super::Context,
    modules::{Source, conditional_path, known_attrs, module_file},
};
use crate::common::{
    exclude::item_attrs,
    imports::{Import, use_tree},
};

pub(crate) type DeclarationKey = (PathBuf, Vec<String>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Type,
    Function,
    Module,
}

#[derive(Default)]
pub(super) struct Module {
    pub(super) imports: Vec<Import>,
    pub(super) unknown: bool,
}

#[derive(Default)]
pub(crate) struct DeclarationIndex {
    pub(super) locations: BTreeMap<(String, Vec<String>), BTreeSet<DeclarationKey>>,
    pub(super) declarations: BTreeMap<DeclarationKey, Option<Kind>>,
    pub(super) modules: BTreeMap<DeclarationKey, Module>,
    pub(super) externs: BTreeMap<PathBuf, BTreeMap<String, Option<PathBuf>>>,
}

impl DeclarationIndex {
    pub(crate) fn build(ctx: &Context<'_>) -> Result<Self> {
        let mut index = Self::default();
        let workspace = fs::canonicalize(ctx.workspace_root)?;
        let mut roots = BTreeMap::<PathBuf, usize>::new();
        for package in ctx.metadata.workspace_packages() {
            for target in &package.targets {
                let Ok(root) = fs::canonicalize(&target.src_path) else {
                    continue;
                };
                *roots.entry(root.clone()).or_default() += 1;
                let mut external = external_roots(ctx, package);
                for library in package.targets.iter().filter(|target| is_library(target)) {
                    if let Ok(library_root) = fs::canonicalize(&library.src_path)
                        && library_root != root
                    {
                        external.insert(library.name.replace('-', "_"), Some(library_root));
                    }
                }
                index.externs.insert(root, external);
            }
        }
        let mut graph = Graph {
            index: &mut index,
            load: |path: &Path| ctx.parsed_file(path),
            workspace: &workspace,
            active: BTreeSet::new(),
        };
        for (root, count) in roots {
            let Some(source) = Source::root(root) else {
                continue;
            };
            if count > 1 {
                graph
                    .index
                    .modules
                    .entry(source.module)
                    .or_default()
                    .unknown = true;
                continue;
            }
            graph.file(&source)?;
        }
        for locations in index
            .locations
            .values()
            .filter(|locations| locations.len() > 1)
        {
            for module in locations {
                index.modules.entry(module.clone()).or_default().unknown = true;
            }
        }
        Ok(index)
    }
}

struct Graph<'a, F> {
    index: &'a mut DeclarationIndex,
    load: F,
    workspace: &'a Path,
    active: BTreeSet<PathBuf>,
}

impl<'ast, F> Graph<'_, F>
where
    F: Fn(&Path) -> Result<Option<&'ast syn::File>>,
{
    fn file(&mut self, source: &Source) -> Result<()> {
        let Some(file) = (self.load)(&source.file)? else {
            self.index
                .modules
                .entry(source.module.clone())
                .or_default()
                .unknown = true;
            return Ok(());
        };
        if !self.active.insert(source.file.clone()) {
            self.index
                .modules
                .entry(source.module.clone())
                .or_default()
                .unknown = true;
            return Ok(());
        }
        self.items(&file.items, source)?;
        if !known_attrs(&file.attrs) {
            self.index
                .modules
                .entry(source.module.clone())
                .or_default()
                .unknown = true;
        }
        self.active.remove(&source.file);
        Ok(())
    }

    fn items(&mut self, items: &[Item], source: &Source) -> Result<()> {
        let rel = source
            .file
            .strip_prefix(self.workspace)
            .unwrap_or(&source.file)
            .to_string_lossy()
            .replace('\\', "/");
        self.index
            .locations
            .entry((rel, source.inline.clone()))
            .or_default()
            .insert(source.module.clone());
        if self
            .index
            .modules
            .insert(source.module.clone(), Module::default())
            .is_some()
        {
            self.index
                .modules
                .entry(source.module.clone())
                .or_default()
                .unknown = true;
        }
        for item in items {
            if let Some((name, kind)) = declaration(item) {
                let mut key = source.module.clone();
                key.1.push(name);
                let value = known_attrs(item_attrs(item)).then_some(kind).flatten();
                if self.index.declarations.insert(key.clone(), value).is_some() {
                    self.index.declarations.insert(key, None);
                }
            }
            match item {
                Item::Use(import) => {
                    let module = self.index.modules.entry(source.module.clone()).or_default();
                    if known_attrs(&import.attrs) {
                        let start = module.imports.len();
                        use_tree(&import.tree, &mut Vec::new(), &mut module.imports);
                        for binding in &mut module.imports[start..] {
                            binding.absolute = import.leading_colon.is_some();
                        }
                    } else {
                        module.unknown = true;
                    }
                }
                Item::ExternCrate(item) => {
                    let module = self.index.modules.entry(source.module.clone()).or_default();
                    if known_attrs(&item.attrs) {
                        let alias = item
                            .rename
                            .as_ref()
                            .map_or(&item.ident, |(_, alias)| alias)
                            .to_string();
                        let own = item.ident == "self";
                        module.imports.push(Import {
                            alias,
                            path: vec![if own {
                                "crate".to_string()
                            } else {
                                item.ident.to_string()
                            }],
                            absolute: !own,
                        });
                    } else {
                        module.unknown = true;
                    }
                }
                Item::Mod(module)
                    if known_attrs(&module.attrs)
                        && !module.attrs.iter().any(|attr| conditional_path(&attr.meta)) =>
                {
                    let child = source.child(module.ident.to_string());
                    if let Some((_, items)) = &module.content {
                        if module.attrs.iter().any(|attr| attr.path().is_ident("path")) {
                            continue;
                        }
                        self.items(items, &child)?;
                    } else if let Some(path) = module_file(module, source)
                        && let Some(child) = child.external(path)
                    {
                        self.file(&child)?;
                    }
                }
                Item::Macro(mac) if mac.ident.is_none() => {
                    self.index
                        .modules
                        .entry(source.module.clone())
                        .or_default()
                        .unknown = true;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn declaration(item: &Item) -> Option<(String, Option<Kind>)> {
    let (ident, kind) = match item {
        Item::Struct(item) => (&item.ident, Some(Kind::Type)),
        Item::Enum(item) => (&item.ident, Some(Kind::Type)),
        Item::Union(item) => (&item.ident, Some(Kind::Type)),
        Item::Fn(item) => (&item.sig.ident, Some(Kind::Function)),
        Item::Mod(item) => (&item.ident, Some(Kind::Module)),
        Item::Type(item) => (&item.ident, None),
        Item::Const(item) => (&item.ident, None),
        Item::Static(item) => (&item.ident, None),
        Item::Trait(item) => (&item.ident, None),
        _ => return None,
    };
    Some((ident.to_string(), kind))
}

fn external_roots(
    ctx: &Context<'_>,
    package: &cargo_metadata::Package,
) -> BTreeMap<String, Option<PathBuf>> {
    let mut out: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    for dependency in &package.dependencies {
        let Some(path) = dependency
            .path
            .as_ref()
            .and_then(|path| fs::canonicalize(path).ok())
        else {
            continue;
        };
        let roots: Vec<_> = ctx
            .metadata
            .workspace_packages()
            .into_iter()
            .filter(|candidate| {
                candidate
                    .manifest_path
                    .parent()
                    .and_then(|parent| fs::canonicalize(parent).ok())
                    .as_ref()
                    == Some(&path)
            })
            .flat_map(|candidate| &candidate.targets)
            .filter(|target| is_library(target))
            .filter_map(|target| fs::canonicalize(&target.src_path).ok())
            .collect();
        if let [root] = roots.as_slice() {
            let name = dependency
                .rename
                .as_ref()
                .unwrap_or(&dependency.name)
                .replace('-', "_");
            out.entry(name)
                .and_modify(|old| {
                    if old.as_ref() != Some(root) {
                        *old = None;
                    }
                })
                .or_insert_with(|| Some(root.clone()));
        }
    }
    out
}

fn is_library(target: &cargo_metadata::Target) -> bool {
    target.is_lib()
        || target.is_rlib()
        || target.is_dylib()
        || target.is_cdylib()
        || target.is_staticlib()
        || target.is_proc_macro()
}
