use std::collections::{BTreeMap, HashMap};

use super::facts::{FnFact, Import, Place};

pub(super) type ItemPath = Vec<String>;
pub(super) type Bindings = HashMap<String, Vec<Ty>>;
pub(super) type Bounds = BTreeMap<String, Vec<syn::Path>>;

#[derive(Clone, Copy)]
pub(super) struct Scope<'a> {
    pub(super) place: &'a Place,
    pub(super) bounds: &'a Bounds,
    pub(super) imports: &'a [Import],
}

impl<'a> From<&'a FnFact> for Scope<'a> {
    fn from(f: &'a FnFact) -> Self {
        Self {
            place: &f.place,
            bounds: &f.generics,
            imports: &f.body.uses,
        }
    }
}

/// A resolved declaration or language shape. Each generic slot retains its cfg
/// alternatives without turning nested arguments into receiver candidates.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Ty {
    Named {
        path: ItemPath,
        args: Vec<Vec<Self>>,
    },
    Slice(Vec<Self>),
    Tuple(Vec<Vec<Self>>),
    /// A symbolic impl parameter used only while matching an owner pattern.
    Parameter(String),
}

impl Ty {
    pub(super) fn named(path: ItemPath, args: Vec<Vec<Self>>) -> Self {
        Self::Named { path, args }
    }

    pub(super) fn path(&self) -> Option<&[String]> {
        match self {
            Self::Named { path, .. } => Some(path),
            Self::Slice(_) | Self::Tuple(_) | Self::Parameter(_) => None,
        }
    }

    pub(super) fn is(&self, path: &[&str]) -> bool {
        self.path()
            .is_some_and(|own| own.iter().map(String::as_str).eq(path.iter().copied()))
    }

    pub(super) fn slot(&self, index: usize) -> Vec<Self> {
        match self {
            Self::Named { args, .. } | Self::Tuple(args) => {
                args.get(index).cloned().unwrap_or_default()
            }
            Self::Slice(elem) if index == 0 => elem.clone(),
            Self::Slice(_) | Self::Parameter(_) => Vec::new(),
        }
    }
}
