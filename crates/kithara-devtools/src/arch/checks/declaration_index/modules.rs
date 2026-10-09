use std::{fs, path::PathBuf};

use syn::{Attribute, Lit, Meta, punctuated::Punctuated};

use super::DeclarationKey;

pub(super) struct Source {
    pub(super) file: PathBuf,
    pub(super) module: DeclarationKey,
    pub(super) inline: Vec<String>,
    pub(super) dir: PathBuf,
    pub(super) path_dir: PathBuf,
}

impl Source {
    pub(super) fn root(file: PathBuf) -> Option<Self> {
        let dir = file.parent()?.to_path_buf();
        Some(Self {
            module: (file.clone(), Vec::new()),
            file,
            inline: Vec::new(),
            path_dir: dir.clone(),
            dir,
        })
    }

    pub(super) fn child(&self, name: String) -> Self {
        let mut module = self.module.clone();
        module.1.push(name.clone());
        let mut inline = self.inline.clone();
        inline.push(name.clone());
        Self {
            file: self.file.clone(),
            module,
            inline,
            dir: self.dir.join(&name),
            path_dir: self.dir.join(name),
        }
    }

    pub(super) fn external(mut self, file: PathBuf) -> Option<Self> {
        let parent = file.parent()?.to_path_buf();
        self.dir = if file.file_name()? == "mod.rs" {
            parent.clone()
        } else {
            parent.join(file.file_stem()?)
        };
        self.path_dir = parent;
        self.file = file;
        self.inline.clear();
        Some(self)
    }
}

pub(super) fn module_file(module: &syn::ItemMod, source: &Source) -> Option<PathBuf> {
    let paths: Vec<_> = module
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("path"))
        .collect();
    let path = match paths.as_slice() {
        [] => {
            let flat = source.dir.join(format!("{}.rs", module.ident));
            let nested = source.dir.join(module.ident.to_string()).join("mod.rs");
            match (flat.is_file(), nested.is_file()) {
                (true, false) => flat,
                (false, true) => nested,
                _ => return None,
            }
        }
        [attr] => {
            let Meta::NameValue(value) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(lit) = &value.value else {
                return None;
            };
            let Lit::Str(path) = &lit.lit else {
                return None;
            };
            source.path_dir.join(path.value())
        }
        _ => return None,
    };
    fs::canonicalize(path).ok()
}

pub(in crate::arch::checks) fn known_attrs(attrs: &[Attribute]) -> bool {
    attrs.iter().all(|attr| known_meta(&attr.meta))
}

pub(super) fn conditional_path(meta: &Meta) -> bool {
    if !meta.path().is_ident("cfg_attr") {
        return false;
    }
    let Meta::List(list) = meta else {
        return true;
    };
    list.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
        .map_or(true, |items| {
            items
                .iter()
                .skip(1)
                .any(|meta| meta.path().is_ident("path") || conditional_path(meta))
        })
}

fn known_meta(meta: &Meta) -> bool {
    if meta.path().is_ident("cfg") {
        let Meta::List(list) = meta else {
            return false;
        };
        return list.parse_args::<Meta>().is_ok();
    }
    if meta.path().is_ident("cfg_attr") {
        let Meta::List(list) = meta else {
            return false;
        };
        return list
            .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
            .is_ok_and(|items| items.len() >= 2 && items.iter().skip(1).all(known_meta));
    }
    meta.path().get_ident().is_some_and(|ident| {
        matches!(
            ident.to_string().as_str(),
            "doc"
                | "cfg"
                | "derive"
                | "repr"
                | "must_use"
                | "non_exhaustive"
                | "allow"
                | "warn"
                | "deny"
                | "forbid"
                | "expect"
                | "test"
                | "path"
                | "feature"
                | "inline"
                | "track_caller"
        )
    })
}
