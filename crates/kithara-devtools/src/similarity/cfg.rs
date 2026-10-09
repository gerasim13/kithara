use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use quote::ToTokens as _;
use syn::{Attribute, Expr, Item, Lit, Meta, Token, punctuated::Punctuated};

/// `cfg` keys that may hold several values in one build; every other keyed
/// atom (`target_os`, `target_arch`, ...) has exactly one value per build.
const MULTI_VALUED_KEYS: &[&str] = &[
    "feature",
    "target_feature",
    "target_family",
    "target_has_atomic",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Atom {
    key: String,
    value: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum Predicate {
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Atom(Atom),
}

impl Predicate {
    pub(super) fn from_attributes(attributes: &[Attribute]) -> Result<Self> {
        let predicates = attributes
            .iter()
            .filter(|attribute| attribute.path().is_ident("cfg"))
            .map(|attribute| Self::parse(&attribute.parse_args::<Meta>()?))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::All(predicates))
    }

    fn parse(meta: &Meta) -> Result<Self> {
        let Meta::List(list) = meta else {
            return Ok(Self::Atom(Self::atom(meta)));
        };
        let children = list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
            .iter()
            .map(Self::parse)
            .collect::<Result<Vec<_>>>()?;
        if list.path.is_ident("all") {
            Ok(Self::All(children))
        } else if list.path.is_ident("any") {
            Ok(Self::Any(children))
        } else if list.path.is_ident("not") && children.len() == 1 {
            let mut children = children.into_iter();
            let Some(child) = children.next() else {
                bail!("cfg not requires one predicate");
            };
            Ok(Self::Not(Box::new(child)))
        } else {
            bail!("unsupported cfg predicate: {}", meta.to_token_stream());
        }
    }

    pub(super) fn and(self, other: Self) -> Self {
        Self::All(vec![self, other])
    }

    fn atom(meta: &Meta) -> Atom {
        Atom {
            key: meta.path().to_token_stream().to_string(),
            value: match meta {
                Meta::NameValue(value) => Some(value.value.to_token_stream().to_string()),
                _ => None,
            },
        }
    }

    fn atoms(&self, atoms: &mut BTreeSet<Atom>) {
        match self {
            Self::All(children) | Self::Any(children) => {
                for child in children {
                    child.atoms(atoms);
                }
            }
            Self::Not(child) => child.atoms(atoms),
            Self::Atom(atom) => {
                atoms.insert(atom.clone());
            }
        }
    }

    fn evaluate(&self, enabled: &BTreeSet<Atom>) -> bool {
        match self {
            Self::All(children) => children.iter().all(|child| child.evaluate(enabled)),
            Self::Any(children) => children.iter().any(|child| child.evaluate(enabled)),
            Self::Not(child) => !child.evaluate(enabled),
            Self::Atom(atom) => enabled.contains(atom),
        }
    }

    pub(super) fn can_coexist(&self, other: &Self) -> bool {
        let mut atoms = BTreeSet::new();
        self.atoms(&mut atoms);
        other.atoms(&mut atoms);
        Self::satisfiable(
            self,
            other,
            &atoms.into_iter().collect::<Vec<_>>(),
            &mut BTreeSet::new(),
        )
    }

    fn satisfiable(
        left: &Self,
        right: &Self,
        atoms: &[Atom],
        enabled: &mut BTreeSet<Atom>,
    ) -> bool {
        let Some((atom, remaining)) = atoms.split_first() else {
            return left.evaluate(enabled) && right.evaluate(enabled);
        };
        if Self::satisfiable(left, right, remaining, enabled) {
            return true;
        }
        if atom.value.is_some()
            && !MULTI_VALUED_KEYS.contains(&atom.key.as_str())
            && enabled.iter().any(|other| {
                other.key == atom.key && other.value.is_some() && other.value != atom.value
            })
        {
            return false;
        }
        enabled.insert(atom.clone());
        let satisfied = Self::satisfiable(left, right, remaining, enabled);
        enabled.remove(atom);
        satisfied
    }

    pub(super) fn is_test_only(&self) -> bool {
        let test = Self::Atom(Self::atom(&syn::parse_quote!(test)));
        !self.can_coexist(&Self::Not(Box::new(test)))
    }
}

pub(super) fn module_predicates(
    files: &BTreeMap<String, syn::File>,
) -> Result<BTreeMap<String, Predicate>> {
    let mut predicates = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    let mut roots = files
        .keys()
        .filter(|path| {
            matches!(
                Path::new(path).file_name().and_then(|name| name.to_str()),
                Some("lib.rs" | "main.rs")
            )
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|path| Path::new(path).components().count());
    for path in roots {
        if predicates.contains_key(path) {
            continue;
        }
        visit_file(
            path,
            Predicate::All(Vec::new()),
            files,
            &mut predicates,
            &mut visiting,
        )?;
    }
    Ok(predicates)
}

fn visit_file(
    path: &str,
    inherited: Predicate,
    files: &BTreeMap<String, syn::File>,
    predicates: &mut BTreeMap<String, Predicate>,
    visiting: &mut BTreeSet<String>,
) -> Result<()> {
    let Some(file) = files.get(path) else {
        return Ok(());
    };
    if !visiting.insert(path.to_string()) {
        bail!("cyclic module declarations in {path}");
    }
    let effective = inherited.and(Predicate::from_attributes(&file.attrs)?);
    predicates
        .entry(path.to_string())
        .and_modify(|previous| {
            *previous = Predicate::Any(vec![previous.clone(), effective.clone()]);
        })
        .or_insert_with(|| effective.clone());
    let path = Path::new(path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let directory = if matches!(
        path.file_stem().and_then(|name| name.to_str()),
        Some("lib" | "main" | "mod")
    ) {
        parent.to_path_buf()
    } else {
        parent.join(path.file_stem().unwrap_or_default())
    };
    visit_modules(
        &file.items,
        &directory,
        parent,
        &effective,
        files,
        predicates,
        visiting,
    )?;
    visiting.remove(&path.to_string_lossy().into_owned());
    Ok(())
}

fn visit_modules(
    items: &[Item],
    directory: &Path,
    path_directory: &Path,
    inherited: &Predicate,
    files: &BTreeMap<String, syn::File>,
    predicates: &mut BTreeMap<String, Predicate>,
    visiting: &mut BTreeSet<String>,
) -> Result<()> {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };
        let effective = inherited
            .clone()
            .and(Predicate::from_attributes(&module.attrs)?);
        let name = module.ident.to_string();
        if let Some((_, items)) = &module.content {
            let directory = directory.join(&name);
            visit_modules(
                items, &directory, &directory, &effective, files, predicates, visiting,
            )?;
            continue;
        }
        let explicit = module.attrs.iter().find_map(|attribute| {
            if !attribute.path().is_ident("path") {
                return None;
            }
            let Meta::NameValue(value) = &attribute.meta else {
                return None;
            };
            let Expr::Lit(value) = &value.value else {
                return None;
            };
            let Lit::Str(value) = &value.lit else {
                return None;
            };
            Some(path_directory.join(value.value()))
        });
        let candidates = explicit.map_or_else(
            || {
                vec![
                    directory.join(format!("{name}.rs")),
                    directory.join(&name).join("mod.rs"),
                ]
            },
            |path| vec![path],
        );
        for candidate in candidates {
            let candidate = normalize(&candidate);
            if files.contains_key(&candidate) {
                visit_file(&candidate, effective.clone(), files, predicates, visiting)?;
            }
        }
    }
    Ok(())
}

fn normalize(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::Predicate;

    fn predicate(attribute: syn::Attribute) -> Predicate {
        Predicate::from_attributes(&[attribute]).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn single_valued_keys_exclude_each_other_but_features_combine() {
        let ios = predicate(syn::parse_quote!(#[cfg(target_os = "ios")]));
        let android = predicate(syn::parse_quote!(#[cfg(target_os = "android")]));
        let first = predicate(syn::parse_quote!(#[cfg(feature = "first")]));
        let second = predicate(syn::parse_quote!(#[cfg(feature = "second")]));

        assert!(!ios.can_coexist(&android));
        assert!(first.can_coexist(&second));
    }
}
