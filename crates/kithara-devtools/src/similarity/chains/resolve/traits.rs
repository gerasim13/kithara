use std::collections::BTreeSet;

use syn::Type;

use super::{Found, Resolver, dedup};
use crate::similarity::chains::{
    facts::FnFact,
    ty::{Bindings, ItemPath, Scope, Ty},
};

impl Resolver<'_> {
    pub(super) fn index_trait_methods(&mut self) {
        let mut methods: BTreeSet<(ItemPath, String)> = BTreeSet::new();
        for (fid, f) in self.facts.fns.iter().enumerate() {
            for tr in self.traits.get(fid).into_iter().flatten() {
                methods.insert((tr.clone(), f.name.clone()));
            }
        }
        for (tr, method) in methods {
            let mut selected = Vec::new();
            for (owner, headers) in &self.impl_traits {
                for (header, traits) in headers {
                    if !traits.contains(&tr) {
                        continue;
                    }
                    let scope = Scope {
                        place: &header.place,
                        bounds: &header.bounds,
                        imports: &[],
                    };
                    let declared = header
                        .trait_name
                        .as_ref()
                        .map(|path| self.named(path, scope, &Bindings::new(), 0))
                        .unwrap_or_default();
                    let overrides: Vec<usize> = self
                        .by_owner
                        .get(owner)
                        .and_then(|methods| methods.get(method.as_str()))
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|fid| {
                            self.fact(*fid).is_some_and(|f| {
                                self.declared_traits(f, &Bindings::new())
                                    .iter()
                                    .any(|ty| declared.contains(ty))
                            })
                        })
                        .collect();
                    if overrides.is_empty() {
                        selected.extend(self.defaults(&tr, &method));
                    } else {
                        selected.extend(overrides);
                    }
                }
            }
            self.trait_impls.insert((tr, method), dedup(selected));
        }
    }

    pub(super) fn defaults(&self, tr: &[String], method: &str) -> Vec<usize> {
        self.by_owner
            .get(tr)
            .and_then(|methods| methods.get(method))
            .into_iter()
            .flatten()
            .copied()
            .filter(|fid| self.fact(*fid).is_some_and(|f| f.default))
            .collect()
    }

    fn declared_traits(&self, f: &FnFact, bindings: &Bindings) -> Vec<Ty> {
        f.trait_name
            .as_ref()
            .map(|path| self.named(path, Scope::from(f), bindings, 0))
            .unwrap_or_default()
    }

    fn trait_instances(&self, ty: &Ty) -> Vec<Ty> {
        let Some(key) = ty.path() else {
            return Vec::new();
        };
        dedup(
            self.impl_traits
                .get(key)
                .into_iter()
                .flatten()
                .flat_map(|(header, _)| {
                    let scope = Scope {
                        place: &header.place,
                        bounds: &header.bounds,
                        imports: &[],
                    };
                    let Some(bindings) = self.match_owner(&header.owner, ty, scope) else {
                        return Vec::new();
                    };
                    let Some(path) = &header.trait_name else {
                        return Vec::new();
                    };
                    self.named(path, scope, &bindings, 0)
                })
                .collect(),
        )
    }

    pub(super) fn traits_of(&self, ty: &Ty) -> BTreeSet<ItemPath> {
        self.trait_instances(ty)
            .iter()
            .filter_map(|tr| tr.path().map(<[String]>::to_vec))
            .collect()
    }

    pub(super) fn returned(
        &self,
        fns: &[usize],
        receiver: Option<&Ty>,
        selector: Option<&[Ty]>,
    ) -> Vec<Ty> {
        dedup(
            fns.iter()
                .flat_map(|fid| {
                    let Some(f) = self.fact(*fid) else {
                        return Vec::new();
                    };
                    let scope = Scope::from(f);
                    let contexts = if let Some(receiver) = receiver
                        && f.default
                    {
                        let mut traits = self.trait_instances(receiver);
                        if receiver
                            .path()
                            .is_some_and(|key| self.trait_keys(*fid).iter().any(|tr| tr == key))
                        {
                            traits.push(receiver.clone());
                        }
                        traits
                            .iter()
                            .filter(|tr| {
                                tr.path().is_some_and(|key| {
                                    self.trait_keys(*fid).iter().any(|own| own == key)
                                }) && selector.is_none_or(|selectors| selectors.contains(tr))
                            })
                            .filter_map(|tr| {
                                f.owner
                                    .as_ref()
                                    .and_then(|owner| self.match_owner(owner, tr, scope))
                            })
                            .collect()
                    } else {
                        let mut bindings = receiver
                            .and_then(|receiver| {
                                if receiver.path().is_some_and(|key| {
                                    self.trait_keys(*fid).iter().any(|tr| tr == key)
                                }) {
                                    let path = f.trait_name.as_ref()?;
                                    let raw = Type::Path(syn::TypePath {
                                        attrs: Vec::new(),
                                        qself: None,
                                        path: path.clone(),
                                    });
                                    self.match_owner(&raw, receiver, scope)
                                } else {
                                    f.owner
                                        .as_ref()
                                        .and_then(|owner| self.match_owner(owner, receiver, scope))
                                }
                            })
                            .unwrap_or_default();
                        if let Some(receiver) = receiver
                            && receiver
                                .path()
                                .is_some_and(|key| self.trait_keys(*fid).iter().any(|tr| tr == key))
                            && let Some(owner) = &f.owner
                        {
                            let owners = self.known(owner, scope, &bindings, 0);
                            bindings.insert("Self".to_owned(), owners);
                        }
                        vec![bindings]
                    };
                    contexts
                        .into_iter()
                        .flat_map(|mut bindings| {
                            if let Some(receiver) = receiver
                                && (f.default || !bindings.contains_key("Self"))
                            {
                                bindings.insert("Self".to_owned(), vec![receiver.clone()]);
                            }
                            f.ret
                                .as_ref()
                                .map(|ret| self.known(ret, scope, &bindings, 0))
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
        )
    }

    pub(super) fn selector(&self, path: &syn::Path, fid: usize) -> Option<Vec<Ty>> {
        if path.segments.len() == 1 {
            return None;
        }
        let f = self.fact(fid)?;
        let mut selector = path.clone();
        selector.segments.pop();
        selector.segments.pop_punct();
        let raw = Type::Path(syn::TypePath {
            attrs: Vec::new(),
            qself: None,
            path: selector,
        });
        Some(self.known(&raw, Scope::from(f), &self.self_bindings(fid), 0))
    }

    pub(super) fn qualified_method(&self, path: &syn::Path, ty: &Ty, fid: usize) -> Found {
        if self.fact(fid).is_none() {
            return Found::default();
        }
        let Some(method) = path.segments.last() else {
            return Found::default();
        };
        if path.segments.len() == 1 {
            return self.methods_of(ty, &method.ident.to_string());
        }
        let traits = self.selector(path, fid).unwrap_or_default();
        let Some(key) = ty.path() else {
            return Found::default();
        };
        let method = method.ident.to_string();
        if traits.contains(ty)
            && self
                .types
                .get(key)
                .into_iter()
                .flatten()
                .any(|def| matches!(def, super::TypeDef::Trait(_)))
        {
            return self.methods_of(ty, &method);
        }
        let fns: Vec<usize> = self
            .by_owner
            .get(key)
            .and_then(|methods| methods.get(method.as_str()))
            .into_iter()
            .flatten()
            .copied()
            .filter(|fid| {
                self.fact(*fid).is_some_and(|f| {
                    let scope = Scope::from(f);
                    let Some(bindings) = f
                        .owner
                        .as_ref()
                        .and_then(|owner| self.match_owner(owner, ty, scope))
                    else {
                        return false;
                    };
                    self.declared_traits(f, &bindings)
                        .iter()
                        .any(|tr| traits.contains(tr))
                })
            })
            .collect();
        if !fns.is_empty() {
            return Found {
                fns,
                dispatch: false,
            };
        }
        let fns = self
            .trait_instances(ty)
            .iter()
            .filter(|tr| traits.contains(tr))
            .filter_map(Ty::path)
            .flat_map(|tr| self.defaults(tr, &method))
            .collect();
        Found {
            fns: dedup(fns),
            dispatch: false,
        }
    }
}
