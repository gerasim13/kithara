use syn::{GenericArgument, PathArguments, Type};

use super::{Resolver, TypeDef, consts, dedup};
use crate::similarity::chains::{
    facts::path_segs,
    ty::{Bindings, Scope, Ty},
};

impl Resolver<'_> {
    pub(super) fn known(
        &self,
        raw: &Type,
        scope: Scope<'_>,
        bindings: &Bindings,
        depth: usize,
    ) -> Vec<Ty> {
        if depth > consts::ALIAS_DEPTH {
            return Vec::new();
        }
        match raw {
            Type::Reference(reference) => self.known(&reference.elem, scope, bindings, depth),
            Type::Ptr(pointer) => self.known(&pointer.elem, scope, bindings, depth),
            Type::Paren(paren) => self.known(&paren.elem, scope, bindings, depth),
            Type::Group(group) => self.known(&group.elem, scope, bindings, depth),
            Type::Slice(slice) => vec![Ty::Slice(self.known(&slice.elem, scope, bindings, depth))],
            Type::Array(array) => vec![Ty::Slice(self.known(&array.elem, scope, bindings, depth))],
            Type::Tuple(tuple) => vec![Ty::Tuple(
                tuple
                    .elems
                    .iter()
                    .map(|raw| self.known(raw, scope, bindings, depth))
                    .collect(),
            )],
            Type::TraitObject(object) => self.bounds(object.bounds.iter(), scope, bindings, depth),
            Type::ImplTrait(object) => self.bounds(object.bounds.iter(), scope, bindings, depth),
            Type::Path(path) if path.qself.is_none() => {
                self.named(&path.path, scope, bindings, depth)
            }
            _ => Vec::new(),
        }
    }

    fn bounds<'a>(
        &self,
        bounds: impl Iterator<Item = &'a syn::TypeParamBound>,
        scope: Scope<'_>,
        bindings: &Bindings,
        depth: usize,
    ) -> Vec<Ty> {
        dedup(
            bounds
                .filter_map(|bound| match bound {
                    syn::TypeParamBound::Trait(tr) => Some(&tr.path),
                    _ => None,
                })
                .flat_map(|path| self.named(path, scope, bindings, depth + 1))
                .collect(),
        )
    }

    pub(super) fn named(
        &self,
        path: &syn::Path,
        scope: Scope<'_>,
        bindings: &Bindings,
        depth: usize,
    ) -> Vec<Ty> {
        let segs = path_segs(path);
        if let [name] = segs.as_slice()
            && path.leading_colon.is_none()
        {
            if let Some(bound) = bindings.get(name) {
                return bound.clone();
            }
            if let Some(bounds) = scope.bounds.get(name) {
                return dedup(
                    bounds
                        .iter()
                        .flat_map(|path| {
                            if depth >= consts::ALIAS_DEPTH {
                                Vec::new()
                            } else {
                                self.named(path, scope, bindings, depth + 1)
                            }
                        })
                        .collect(),
                );
            }
        }
        let args = path
            .segments
            .last()
            .map(|last| match &last.arguments {
                PathArguments::AngleBracketed(args) => args
                    .args
                    .iter()
                    .filter_map(|arg| match arg {
                        GenericArgument::Type(raw) => Some(self.known(raw, scope, bindings, depth)),
                        GenericArgument::AssocType(associated) if associated.ident == "Item" => {
                            Some(self.known(&associated.ty, scope, bindings, depth))
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for key in self.path(path, scope, 0) {
            let ty = Ty::named(key.clone(), args.clone());
            let Some(defs) = self.types.get(&key) else {
                if key.first().is_some_and(|head| head == "std") {
                    out.push(ty);
                }
                continue;
            };
            for def in defs {
                match *def {
                    TypeDef::Alias(alias) if depth < consts::ALIAS_DEPTH => {
                        out.extend(self.known(
                            &alias.ty,
                            def.scope(),
                            &Self::type_bindings(*def, &ty),
                            depth + 1,
                        ));
                    }
                    TypeDef::Alias(_) => {}
                    _ => out.push(ty.clone()),
                }
            }
        }
        dedup(out)
    }

    pub(super) fn type_bindings(def: TypeDef<'_>, ty: &Ty) -> Bindings {
        let mut bindings: Bindings = def
            .params()
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), ty.slot(index)))
            .collect();
        bindings.insert("Self".to_owned(), vec![ty.clone()]);
        bindings
    }

    pub(super) fn match_owner(
        &self,
        pattern: &Type,
        ty: &Ty,
        scope: Scope<'_>,
    ) -> Option<Bindings> {
        let parameters = scope
            .bounds
            .keys()
            .filter(|name| *name != "Self")
            .map(|name| (name.clone(), vec![Ty::Parameter(name.clone())]))
            .collect();
        let contexts = self
            .known(pattern, scope, &parameters, 0)
            .iter()
            .filter_map(|pattern| {
                let mut bindings = Bindings::new();
                if !Self::match_type(pattern, ty, &mut bindings) {
                    return None;
                }
                Some(bindings)
            })
            .collect();
        let mut bindings = Self::merge_matches(contexts)?;
        bindings.insert("Self".to_owned(), vec![ty.clone()]);
        Some(bindings)
    }

    fn match_type(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> bool {
        match (pattern, actual) {
            (
                Ty::Named { path, args },
                Ty::Named {
                    path: own,
                    args: slots,
                },
            ) if path == own => {
                args.len() == slots.len()
                    && args
                        .iter()
                        .zip(slots)
                        .all(|(pattern, actual)| Self::match_slot(pattern, actual, bindings))
            }
            (Ty::Slice(pattern), Ty::Slice(actual)) => Self::match_slot(pattern, actual, bindings),
            (Ty::Tuple(pattern), Ty::Tuple(actual)) => {
                pattern.len() == actual.len()
                    && pattern
                        .iter()
                        .zip(actual)
                        .all(|(pattern, actual)| Self::match_slot(pattern, actual, bindings))
            }
            _ => false,
        }
    }

    fn match_slot(pattern: &[Ty], actual: &[Ty], bindings: &mut Bindings) -> bool {
        if let [Ty::Parameter(name)] = pattern {
            if let Some(previous) = bindings.get(name) {
                return previous == actual;
            }
            bindings.insert(name.clone(), actual.to_vec());
            return true;
        }
        if pattern.is_empty() {
            return false;
        }
        let mut contexts = Vec::new();
        for pattern in pattern {
            for actual in actual {
                let mut context = bindings.clone();
                if Self::match_type(pattern, actual, &mut context) {
                    contexts.push(context);
                }
            }
        }
        let Some(merged) = Self::merge_matches(contexts) else {
            return false;
        };
        *bindings = merged;
        true
    }

    fn merge_matches(contexts: Vec<Bindings>) -> Option<Bindings> {
        let mut contexts = contexts.into_iter();
        let mut bindings = contexts.next()?;
        for context in contexts {
            for (name, values) in context {
                let existing = bindings.entry(name).or_default();
                existing.extend(values);
                *existing = dedup(std::mem::take(existing));
            }
        }
        Some(bindings)
    }
}
