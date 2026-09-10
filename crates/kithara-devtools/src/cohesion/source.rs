use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use syn::{
    Attribute, Expr, ImplItem, Item, ItemImpl, Lit, Meta, Type, UseTree, punctuated::Punctuated,
    spanned::Spanned,
};

use crate::common::{
    exclude::{attrs_are_test_only, item_attrs},
    parse::parse_file,
};

pub(super) struct Definition {
    pub(super) location: String,
    pub(super) module: String,
    pub(super) is_enum: bool,
}

#[derive(Default)]
pub(super) struct Source {
    pub(super) definitions: BTreeMap<String, Definition>,
    pub(super) notes: BTreeSet<String>,
    pub(super) implementations: Vec<Implementation>,
    imports: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    root: PathBuf,
    include_tests: bool,
}

pub(super) struct Implementation {
    pub(super) item: ItemImpl,
    pub(super) location: String,
    pub(super) module: String,
}

impl Source {
    fn definition(&mut self, module: &str, name: &str, location: String, is_enum: bool) {
        let name = join(module, name);
        if self.definitions.contains_key(&name) {
            self.notes.insert(format!(
                "{name}: multiple source definitions (cfg alternatives combined)"
            ));
        }
        self.definitions.insert(
            name,
            Definition {
                is_enum,
                location,
                module: module.to_owned(),
            },
        );
    }

    fn file(&mut self, path: &Path, module: &str, active: &mut BTreeSet<PathBuf>) -> Result<()> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("resolve LCOM4 source {}", path.display()))?;
        if !active.insert(canonical.clone()) {
            bail!("recursive LCOM4 module source: {}", path.display());
        }
        let file = parse_file(path)?;
        if !self.include_tests && attrs_are_test_only(&file.attrs) {
            active.remove(&canonical);
            return Ok(());
        }
        let parent = path.parent().context("LCOM4 source has no parent")?;
        let directory = match path.file_stem().and_then(|stem| stem.to_str()) {
            Some("lib" | "main" | "mod") => parent.to_path_buf(),
            Some(stem) if !module.is_empty() => parent.join(stem),
            _ => parent.to_path_buf(),
        };
        self.items(path, &directory, module, &file.items, active)?;
        active.remove(&canonical);
        Ok(())
    }

    fn items(
        &mut self,
        file: &Path,
        directory: &Path,
        module: &str,
        items: &[Item],
        active: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        for item in items {
            if !self.include_tests && attrs_are_test_only(item_attrs(item)) {
                continue;
            }
            let location = format!(
                "{}:{}",
                file.strip_prefix(&self.root).unwrap_or(file).display(),
                item.span().start().line
            );
            match item {
                Item::Struct(value) => {
                    self.definition(module, &value.ident.to_string(), location, false);
                }
                Item::Enum(value) => {
                    self.definition(module, &value.ident.to_string(), location, true);
                }
                Item::Impl(value) => self.implementations.push(Implementation {
                    location,
                    item: value.clone(),
                    module: module.to_owned(),
                }),
                Item::Use(value) => {
                    let mut imports = Vec::new();
                    collect_imports(&value.tree, "", &mut imports);
                    for (name, path) in imports {
                        self.imports
                            .entry(module.to_owned())
                            .or_default()
                            .entry(name)
                            .or_default()
                            .insert(path);
                    }
                }
                Item::Mod(value) => {
                    let child = join(module, &value.ident.to_string());
                    if let Some((_, items)) = &value.content {
                        self.items(
                            file,
                            &directory.join(value.ident.to_string()),
                            &child,
                            items,
                            active,
                        )?;
                    } else {
                        for path in
                            module_files(file, directory, &value.ident.to_string(), &value.attrs)?
                        {
                            if path.is_file() {
                                self.file(&path, &child, active)?;
                            } else {
                                self.notes.insert(format!(
                                    "{location}: module source unavailable: {}",
                                    path.display()
                                ));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn owner(&self, implementation: &Implementation) -> Option<String> {
        let Type::Path(path) = implementation.item.self_ty.as_ref() else {
            return None;
        };
        if path.qself.is_some() || path.path.leading_colon.is_some() {
            return None;
        }
        let path = path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let owner = self.resolve(&implementation.module, &path, &mut BTreeSet::new())?;
        self.definitions.contains_key(&owner).then_some(owner)
    }

    pub(super) fn read(root: &Path, entry: &Path, include_tests: bool) -> Result<Self> {
        let mut source = Self {
            root: root.to_path_buf(),
            include_tests,
            ..Self::default()
        };
        source.file(entry, "", &mut BTreeSet::new())?;
        Ok(source)
    }

    fn resolve(&self, module: &str, path: &str, visited: &mut BTreeSet<String>) -> Option<String> {
        if !visited.insert(format!("{module}|{path}")) {
            return None;
        }
        let mut parts = path.split("::").collect::<Vec<_>>();
        let first = *parts.first()?;
        if first == "crate" {
            return self.resolve_absolute(&parts[1..].join("::"), visited);
        }
        if first == "self" {
            return self.resolve_absolute(&join(module, &parts[1..].join("::")), visited);
        }
        if first == "super" {
            let mut parents = module
                .split("::")
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            while parts.first() == Some(&"super") {
                parents.pop()?;
                parts.remove(0);
            }
            return self.resolve_absolute(&join(&parents.join("::"), &parts.join("::")), visited);
        }
        if let Some(paths) = self
            .imports
            .get(module)
            .and_then(|imports| imports.get(first))
        {
            if paths.len() != 1 {
                return None;
            }
            let imported = paths.first()?;
            return self.resolve(module, &join(imported, &parts[1..].join("::")), visited);
        }
        self.resolve_absolute(&join(module, path), visited)
    }

    fn resolve_absolute(&self, path: &str, visited: &mut BTreeSet<String>) -> Option<String> {
        if self.definitions.contains_key(path) {
            return Some(path.to_owned());
        }
        let (module, name) = path.rsplit_once("::").unwrap_or(("", path));
        let paths = self.imports.get(module)?.get(name)?;
        if paths.len() != 1 {
            return None;
        }
        self.resolve(module, paths.first()?, visited)
    }
}

fn module_files(
    file: &Path,
    directory: &Path,
    name: &str,
    attrs: &[Attribute],
) -> Result<Vec<PathBuf>> {
    let parent = file.parent().context("module source has no parent")?;
    let mut paths = BTreeSet::new();
    let mut conditional = false;
    for attr in attrs {
        if let Some(path) = declared_path(&attr.meta) {
            paths.insert(parent.join(path));
        }
        if let Meta::List(list) = &attr.meta
            && list.path.is_ident("cfg_attr")
        {
            let nested =
                list.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?;
            for meta in nested.iter().skip(1) {
                if let Some(path) = declared_path(meta) {
                    paths.insert(parent.join(path));
                    conditional = true;
                }
            }
        }
    }
    if paths.is_empty() || conditional {
        let flat = directory.join(format!("{name}.rs"));
        let nested = directory.join(name).join("mod.rs");
        match (flat.is_file(), nested.is_file()) {
            (true, true) => bail!(
                "ambiguous module source: {} and {}",
                flat.display(),
                nested.display()
            ),
            (true, false) => {
                paths.insert(flat);
            }
            (false, true) => {
                paths.insert(nested);
            }
            (false, false) if paths.is_empty() => {
                paths.insert(flat);
            }
            (false, false) => {}
        }
    }
    Ok(paths.into_iter().collect())
}

fn declared_path(meta: &Meta) -> Option<String> {
    if let Meta::NameValue(value) = meta
        && value.path.is_ident("path")
        && let Expr::Lit(value) = &value.value
        && let Lit::Str(value) = &value.lit
    {
        Some(value.value())
    } else {
        None
    }
}

fn collect_imports(tree: &UseTree, prefix: &str, output: &mut Vec<(String, String)>) {
    match tree {
        UseTree::Path(path) => {
            collect_imports(&path.tree, &join(prefix, &path.ident.to_string()), output);
        }
        UseTree::Name(name) if name.ident == "self" => {
            if let Some(name) = prefix.rsplit("::").next() {
                output.push((name.to_owned(), prefix.to_owned()));
            }
        }
        UseTree::Name(name) => output.push((
            name.ident.to_string(),
            join(prefix, &name.ident.to_string()),
        )),
        UseTree::Rename(name) => output.push((
            name.rename.to_string(),
            join(prefix, &name.ident.to_string()),
        )),
        UseTree::Group(group) => {
            for tree in &group.items {
                collect_imports(tree, prefix, output);
            }
        }
        UseTree::Glob(_) => {}
    }
}

pub(super) fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else if name.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}::{name}")
    }
}

pub(super) fn methods(item: &ItemImpl, tests: bool) -> impl Iterator<Item = &syn::ImplItemFn> {
    item.items.iter().filter_map(move |item| match item {
        ImplItem::Fn(method)
            if method.sig.receiver().is_some()
                && (tests || !attrs_are_test_only(&method.attrs)) =>
        {
            Some(method)
        }
        _ => None,
    })
}
