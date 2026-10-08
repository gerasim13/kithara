use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::Hash,
};

use syn::Type;

use super::{
    super::{
        arms::TraitMethod,
        body::{Desc, Local, Site, SiteKind},
        facts::{
            AliasFact, EnumFact, Facts, FnFact, ImplFact, Import, Place, StructFact, TraitFact,
            path_segs,
        },
        standard,
        ty::{Bindings, ItemPath, Scope, Ty},
    },
    consts,
};

#[derive(Clone, Copy)]
pub(super) enum TypeDef<'f> {
    Struct(&'f StructFact),
    Enum(&'f EnumFact),
    Trait(&'f TraitFact),
    Alias(&'f AliasFact),
}

impl<'f> TypeDef<'f> {
    fn name(self) -> &'f str {
        match self {
            Self::Struct(data) => &data.name,
            Self::Enum(data) => &data.name,
            Self::Trait(data) => &data.name,
            Self::Alias(data) => &data.name,
        }
    }

    pub(super) fn params(self) -> &'f [String] {
        match self {
            Self::Struct(data) => &data.params,
            Self::Enum(data) => &data.params,
            Self::Trait(data) => &data.params,
            Self::Alias(data) => &data.params,
        }
    }

    pub(super) fn scope(self) -> Scope<'f> {
        let (place, bounds) = match self {
            Self::Struct(data) => (&data.place, &data.bounds),
            Self::Enum(data) => (&data.place, &data.bounds),
            Self::Trait(data) => (&data.place, &data.bounds),
            Self::Alias(data) => (&data.place, &data.bounds),
        };
        Scope {
            place,
            bounds,
            imports: &[],
        }
    }
}

pub(in super::super) type Variant = (ItemPath, String);
type TraitImpl<'f> = (&'f ImplFact, Vec<ItemPath>);
type PathMemo = HashMap<(ItemPath, usize, bool), Vec<ItemPath>>;

#[derive(Debug, Default)]
pub(in super::super) struct Targets {
    pub(in super::super) built: Vec<ItemPath>,
    pub(in super::super) fns: Vec<usize>,
    pub(in super::super) variants: Vec<Variant>,
}

#[derive(Default)]
pub(super) struct Found {
    pub(super) fns: Vec<usize>,
    pub(super) dispatch: bool,
}

#[derive(Default)]
struct PathFound {
    found: Found,
    owners: Vec<Ty>,
    types: Vec<Ty>,
    variants: Vec<Variant>,
}

pub(in super::super) struct Resolver<'f> {
    pub(super) facts: &'f Facts,
    pub(super) by_owner: HashMap<ItemPath, HashMap<&'f str, Vec<usize>>>,
    pub(super) free: HashMap<ItemPath, Vec<usize>>,
    pub(super) impl_traits: HashMap<ItemPath, Vec<TraitImpl<'f>>>,
    pub(super) trait_impls: HashMap<(ItemPath, String), Vec<usize>>,
    pub(super) types: HashMap<ItemPath, Vec<TypeDef<'f>>>,
    pub(super) uses: HashMap<ItemPath, Vec<&'f Import>>,
    pub(super) crates: HashSet<&'f str>,
    pub(super) modules: HashSet<ItemPath>,
    pub(super) path_memo: RefCell<PathMemo>,
    pub(super) traits: Vec<Vec<ItemPath>>,
    derefs: HashMap<ItemPath, Vec<&'f ImplFact>>,
    memo: HashMap<(usize, *const Desc), Option<Vec<Ty>>>,
    owners: Vec<Vec<ItemPath>>,
    self_bindings: Vec<Bindings>,
    default_methods: HashSet<&'f str>,
    max_dyn: usize,
}

impl<'f> Resolver<'f> {
    pub(in super::super) fn new(facts: &'f Facts, max_dyn: usize) -> Self {
        let mut resolver = Self {
            facts,
            max_dyn,
            by_owner: HashMap::new(),
            derefs: HashMap::new(),
            free: HashMap::new(),
            impl_traits: HashMap::new(),
            memo: HashMap::new(),
            trait_impls: HashMap::new(),
            types: HashMap::new(),
            uses: HashMap::new(),
            modules: HashSet::new(),
            path_memo: RefCell::new(HashMap::new()),
            crates: HashSet::new(),
            owners: Vec::new(),
            self_bindings: Vec::new(),
            default_methods: facts
                .fns
                .iter()
                .filter(|fact| fact.default)
                .map(|fact| fact.name.as_str())
                .collect(),
            traits: Vec::new(),
        };
        resolver.index();
        resolver
    }

    fn add_module(&mut self, place: &'f Place) {
        self.crates.insert(&place.krate);
        for len in 0..=place.module.len() {
            if let Some(prefix) = place.module.get(..len) {
                self.modules.insert(module_key(&place.krate, prefix));
            }
        }
    }

    fn autoderef<T>(&self, ty: &Ty, lookup: impl Fn(&Ty) -> Option<T>) -> Vec<T> {
        let mut level = vec![ty.clone()];
        let mut seen: HashSet<Ty> = level.iter().cloned().collect();
        for _ in 0..=consts::DEREF_DEPTH {
            let hits: Vec<T> = level.iter().filter_map(&lookup).collect();
            if !hits.is_empty() {
                return hits;
            }
            level = level
                .iter()
                .flat_map(|step| self.deref_targets(step))
                .filter(|target| seen.insert(target.clone()))
                .collect();
        }
        Vec::new()
    }

    fn call_type(
        &mut self,
        path: &syn::Path,
        arg: Option<&'f Desc>,
        fid: usize,
        depth: usize,
    ) -> Vec<Ty> {
        let Some(f) = self.fact(fid) else {
            return Vec::new();
        };
        let found = self.resolve_path(path, fid);
        if !found.found.fns.is_empty() {
            let fns = self.called(found.found);
            if found.owners.is_empty() {
                return self.returned(&fns, None, None);
            }
            return dedup(
                found
                    .owners
                    .iter()
                    .flat_map(|owner| self.returned(&fns, Some(owner), None))
                    .collect(),
            );
        }
        let arg = arg
            .map(|arg| self.rtype(arg, fid, depth + 1))
            .unwrap_or_default();
        let mut out = Vec::new();
        for key in self.path(path, Scope::from(f), 0) {
            if let Some(types) = standard::constructed(&key, arg.clone()) {
                out.extend(types);
            }
        }
        if !out.is_empty() {
            return dedup(out);
        }
        out.extend(found.types);
        if !found.variants.is_empty() {
            out.extend(found.owners);
        }
        dedup(out)
    }

    fn called(&self, found: Found) -> Vec<usize> {
        let fns = dedup(found.fns);
        if found.dispatch && fns.len() > self.max_dyn {
            Vec::new()
        } else {
            fns
        }
    }

    fn closure_type(&self, ty: &Ty, method: &str, arg: usize, input: usize) -> Vec<Ty> {
        self.autoderef(ty, |step| {
            let found = self.methods_of(step, method);
            if found.fns.is_empty() {
                return standard::closure(step, method, arg, input);
            }
            Some(dedup(
                self.called(found)
                    .iter()
                    .flat_map(|fid| {
                        let Some(fact) = self.fact(*fid) else {
                            return Vec::new();
                        };
                        let Some(raw) = fact.inputs.get(arg) else {
                            return Vec::new();
                        };
                        let Some(owner) = &fact.owner else {
                            return Vec::new();
                        };
                        let scope = Scope::from(fact);
                        let Some(bindings) = self.match_owner(owner, step, scope) else {
                            return Vec::new();
                        };
                        self.known(raw, scope, &bindings, 0)
                            .iter()
                            .flat_map(|callable| standard::callback_input(callable, input))
                            .collect()
                    })
                    .collect(),
            ))
        })
        .into_iter()
        .flatten()
        .collect()
    }

    fn deref_targets(&self, ty: &Ty) -> Vec<Ty> {
        if let Some(standard) = standard::deref(ty) {
            return standard;
        }
        let Some(key) = ty.path() else {
            return Vec::new();
        };
        self.derefs
            .get(key)
            .into_iter()
            .flatten()
            .flat_map(|header| {
                let scope = Scope {
                    place: &header.place,
                    bounds: &header.bounds,
                    imports: &[],
                };
                let Some(bindings) = self.match_owner(&header.owner, ty, scope) else {
                    return Vec::new();
                };
                header
                    .target
                    .as_ref()
                    .map(|target| self.known(target, scope, &bindings, 0))
                    .unwrap_or_default()
            })
            .collect()
    }

    fn enum_variant(&self, ty: &Ty, variant: &str) -> Option<Variant> {
        let key = ty.path()?;
        self.types.get(key).into_iter().flatten().any(|def| {
            matches!(def, TypeDef::Enum(data) if data.variants.iter().any(|name| name == variant))
        }).then(|| (key.to_vec(), variant.to_owned()))
    }

    pub(super) fn fact(&self, fid: usize) -> Option<&'f FnFact> {
        self.facts.fns.get(fid)
    }

    fn field_type(&self, ty: &Ty, field: &str) -> Option<Vec<Ty>> {
        if let Ty::Tuple(parts) = ty {
            return field
                .parse::<usize>()
                .ok()
                .and_then(|index| parts.get(index).cloned());
        }
        let key = ty.path()?;
        let mut out = None;
        for def in self.types.get(key).into_iter().flatten().copied() {
            if let TypeDef::Struct(data) = def
                && let Some(raw) = data.fields.get(field)
            {
                let bindings = Self::type_bindings(def, ty);
                out.get_or_insert_with(Vec::new)
                    .extend(self.known(raw, def.scope(), &bindings, 0));
            }
        }
        out
    }

    fn index(&mut self) {
        let facts = self.facts;
        let defs = facts
            .structs
            .iter()
            .map(TypeDef::Struct)
            .chain(facts.enums.iter().map(TypeDef::Enum))
            .chain(facts.traits.iter().map(TypeDef::Trait))
            .chain(facts.aliases.iter().map(TypeDef::Alias));
        for def in defs {
            let place = def.scope().place;
            self.add_module(place);
            self.types
                .entry(identity(place, def.name()))
                .or_default()
                .push(def);
        }
        for fact in &facts.uses {
            let key = module_key(&fact.krate, &fact.module);
            self.uses.entry(key).or_default().push(&fact.import);
        }
        for (fid, f) in facts.fns.iter().enumerate() {
            self.add_module(&f.place);
            if f.owner.is_none() {
                self.free
                    .entry(identity(&f.place, &f.name))
                    .or_default()
                    .push(fid);
            }
        }
        for (fid, f) in facts.fns.iter().enumerate() {
            let scope = Scope::from(f);
            let types = f
                .owner
                .as_ref()
                .map(|ty| self.known(ty, scope, &Bindings::new(), 0))
                .unwrap_or_default();
            let owners: Vec<ItemPath> = types
                .iter()
                .filter_map(|ty| ty.path().map(<[String]>::to_vec))
                .collect();
            let traits = f
                .trait_name
                .as_ref()
                .map(|path| self.path(path, scope, 0))
                .unwrap_or_default();
            for owner in &owners {
                self.by_owner
                    .entry(owner.clone())
                    .or_default()
                    .entry(&f.name)
                    .or_default()
                    .push(fid);
            }
            self.owners.push(dedup(owners));
            self.self_bindings.push(if f.owner.is_some() {
                Bindings::from([("Self".to_owned(), types)])
            } else {
                Bindings::new()
            });
            self.traits.push(dedup(traits));
        }
        for header in &facts.impls {
            let scope = Scope {
                place: &header.place,
                bounds: &header.bounds,
                imports: &[],
            };
            let owners = self.known(&header.owner, scope, &Bindings::new(), 0);
            let traits = header
                .trait_name
                .as_ref()
                .map(|path| self.path(path, scope, 0))
                .unwrap_or_default();
            for owner in owners.iter().filter_map(Ty::path) {
                self.impl_traits
                    .entry(owner.to_vec())
                    .or_default()
                    .push((header, traits.clone()));
                for tr in &traits {
                    if tr.iter().map(String::as_str).eq(["std", "ops", "Deref"])
                        && header.target.is_some()
                    {
                        self.derefs.entry(owner.to_vec()).or_default().push(header);
                    }
                }
            }
        }
        self.index_trait_methods();
    }

    fn method_return(&self, ty: &Ty, method: &str, scalar: bool) -> Option<Vec<Ty>> {
        let found = self.methods_of(ty, method);
        if !found.fns.is_empty() {
            return Some(self.returned(&self.called(found), Some(ty), None));
        }
        if method == "clone"
            && ty.path().is_some_and(|key| {
                self.types.get(key).into_iter().flatten().any(|def| {
                    let derives = match def {
                        TypeDef::Struct(data) => &data.derives,
                        TypeDef::Enum(data) => &data.derives,
                        _ => return false,
                    };
                    derives.iter().any(|path| {
                        self.path(path, def.scope(), 0).iter().any(|path| {
                            path.iter()
                                .map(String::as_str)
                                .eq(["std", "clone", "Clone"])
                        })
                    })
                })
            })
        {
            return Some(vec![ty.clone()]);
        }
        standard::returned(ty, method, scalar)
    }

    pub(super) fn methods_of(&self, ty: &Ty, method: &str) -> Found {
        let Some(key) = ty.path() else {
            return Found::default();
        };
        if self
            .types
            .get(key)
            .into_iter()
            .flatten()
            .any(|def| matches!(def, TypeDef::Trait(_)))
        {
            let fns = self
                .trait_impls
                .get(&(key.to_vec(), method.to_owned()))
                .into_iter()
                .flatten()
                .copied()
                .filter(|fid| {
                    self.fact(*fid).is_some_and(|f| {
                        if f.default {
                            f.owner.as_ref().is_some_and(|owner| {
                                self.match_owner(owner, ty, Scope::from(f)).is_some()
                            })
                        } else {
                            f.trait_name.as_ref().is_some_and(|path| {
                                let raw = Type::Path(syn::TypePath {
                                    attrs: Vec::new(),
                                    qself: None,
                                    path: path.clone(),
                                });
                                self.match_owner(&raw, ty, Scope::from(f)).is_some()
                            })
                        }
                    })
                })
                .collect();
            return Found {
                fns,
                dispatch: true,
            };
        }
        let fns: Vec<usize> = self
            .by_owner
            .get(key)
            .and_then(|methods| methods.get(method))
            .into_iter()
            .flatten()
            .copied()
            .filter(|fid| {
                self.fact(*fid).is_some_and(|f| {
                    f.owner
                        .as_ref()
                        .is_some_and(|owner| self.match_owner(owner, ty, Scope::from(f)).is_some())
                })
            })
            .collect();
        let inherent: Vec<usize> = fns
            .iter()
            .copied()
            .filter(|fid| self.trait_keys(*fid).is_empty())
            .collect();
        if !inherent.is_empty() {
            return Found {
                fns: inherent,
                dispatch: false,
            };
        }
        if !fns.is_empty() {
            return Found {
                fns,
                dispatch: false,
            };
        }
        if !self.default_methods.contains(method) {
            return Found::default();
        }
        let fns = self
            .traits_of(ty)
            .iter()
            .flat_map(|tr| self.defaults(tr, method))
            .collect();
        Found {
            fns: dedup(fns),
            dispatch: false,
        }
    }

    pub(in super::super) fn owner_keys(&self, fid: usize) -> &[ItemPath] {
        self.owners.get(fid).map_or(&[], Vec::as_slice)
    }

    fn payload_type(
        &mut self,
        of: &'f Desc,
        variant: &syn::Path,
        index: &str,
        fid: usize,
        depth: usize,
    ) -> Vec<Ty> {
        let Some(f) = self.fact(fid) else {
            return Vec::new();
        };
        let Some(last) = variant.segments.last() else {
            return Vec::new();
        };
        let name = last.ident.to_string();
        let mut prefix = variant.clone();
        prefix.segments.pop();
        prefix.segments.pop_punct();
        let scope = Scope::from(f);
        let mut named = self.path(variant, scope, 0);
        if !prefix.segments.is_empty() {
            let raw = Type::Path(syn::TypePath {
                attrs: Vec::new(),
                qself: None,
                path: prefix.clone(),
            });
            named.extend(
                self.known(&raw, scope, &self.self_bindings(fid), 0)
                    .iter()
                    .filter_map(|ty| ty.path().map(<[String]>::to_vec)),
            );
        }
        let mut out = Vec::new();
        for ty in self.rtype(of, fid, depth + 1) {
            let Some(key) = ty.path() else {
                continue;
            };
            if !named.is_empty()
                && !named.iter().any(|named| {
                    named.as_slice() == key
                        || named.split_last().is_some_and(|(_, prefix)| prefix == key)
                })
            {
                continue;
            }
            if !prefix.segments.is_empty() && named.is_empty() {
                continue;
            }
            if let Some(payload) = standard::payload(&ty, &name, index) {
                out.extend(payload);
                continue;
            }
            for def in self.types.get(key).into_iter().flatten().copied() {
                let raw = match def {
                    TypeDef::Enum(data) => {
                        data.payload.get(&name).and_then(|fields| fields.get(index))
                    }
                    TypeDef::Struct(data) if name == def.name() => data.fields.get(index),
                    _ => None,
                };
                if let Some(raw) = raw {
                    out.extend(self.known(raw, def.scope(), &Self::type_bindings(def, &ty), 0));
                }
            }
        }
        dedup(out)
    }

    fn resolve_method(
        &mut self,
        method: &str,
        recv: Option<&'f Desc>,
        fid: usize,
        depth: usize,
    ) -> Found {
        let types = recv
            .map(|recv| self.rtype(recv, fid, depth))
            .unwrap_or_default();
        let mut found = Found::default();
        for ty in types {
            for hit in self.autoderef(&ty, |step| {
                let found = self.methods_of(step, method);
                if !found.fns.is_empty() {
                    Some(found)
                } else {
                    standard::returned(step, method, true).map(|_| Found::default())
                }
            }) {
                found.dispatch |= hit.dispatch;
                found.fns.extend(hit.fns);
            }
        }
        found.fns = dedup(found.fns);
        found
    }

    fn resolve_path(&self, path: &syn::Path, fid: usize) -> PathFound {
        let Some(f) = self.fact(fid) else {
            return PathFound::default();
        };
        let scope = Scope::from(f);
        let segs = path_segs(path);
        let Some((name, _)) = segs.split_last() else {
            return PathFound::default();
        };
        let mut out = PathFound::default();
        for key in self.path(path, scope, 0) {
            out.found
                .fns
                .extend(self.free.get(&key).into_iter().flatten().copied());
            if let Some((variant, prefix)) = key.split_last() {
                let owner = Ty::named(prefix.to_vec(), Vec::new());
                if let Some(key) = self.enum_variant(&owner, variant) {
                    out.variants.push(key);
                    out.owners.push(owner);
                }
            }
        }
        let mut owner_path = path.clone();
        if path.segments.len() > 1 {
            owner_path.segments.pop();
            owner_path.segments.pop_punct();
            let raw = Type::Path(syn::TypePath {
                attrs: Vec::new(),
                qself: None,
                path: owner_path,
            });
            out.owners
                .extend(self.known(&raw, scope, &self.self_bindings(fid), 0));
            for owner in &out.owners {
                let found = self.methods_of(owner, name);
                out.found.fns.extend(found.fns);
                out.found.dispatch |= found.dispatch;
                if let Some(key) = self.enum_variant(owner, name) {
                    out.variants.push(key);
                }
            }
        }
        let raw = Type::Path(syn::TypePath {
            attrs: Vec::new(),
            qself: None,
            path: path.clone(),
        });
        out.types = self.known(&raw, scope, &self.self_bindings(fid), 0);
        out.found.fns = dedup(out.found.fns);
        out.owners = dedup(out.owners);
        out.variants = dedup(out.variants);
        out
    }

    fn rtype(&mut self, desc: &'f Desc, fid: usize, depth: usize) -> Vec<Ty> {
        if depth > consts::TYPE_DEPTH {
            return Vec::new();
        }
        let key = (fid, std::ptr::from_ref(desc));
        if let Some(found) = self.memo.get(&key) {
            return found.clone().unwrap_or_default();
        }
        self.memo.insert(key, None);
        let found = self.rtype_of(desc, fid, depth);
        self.memo.insert(key, Some(found.clone()));
        found
    }

    fn rtype_of(&mut self, desc: &'f Desc, fid: usize, depth: usize) -> Vec<Ty> {
        let Some(f) = self.fact(fid) else {
            return Vec::new();
        };
        match desc {
            Desc::SelfValue => self
                .self_bindings(fid)
                .get("Self")
                .cloned()
                .unwrap_or_default(),
            Desc::Var(id) => match f.body.locals.get(*id) {
                Some(Local::Ty(raw)) => {
                    self.known(raw, Scope::from(f), &self.self_bindings(fid), 0)
                }
                Some(Local::From(from)) => self.rtype(from, fid, depth),
                None => Vec::new(),
            },
            Desc::Ty(raw) => self.known(raw, Scope::from(f), &self.self_bindings(fid), 0),
            Desc::Field { of, field } => {
                let types = self.rtype(of, fid, depth + 1);
                dedup(
                    types
                        .iter()
                        .flat_map(|ty| self.autoderef(ty, |step| self.field_type(step, field)))
                        .flatten()
                        .collect(),
                )
            }
            Desc::Ret { of, method, arg } => {
                let scalar = arg
                    .as_deref()
                    .is_some_and(|arg| self.scalar_index(arg, fid, depth + 1));
                let types = self.rtype(of, fid, depth + 1);
                dedup(
                    types
                        .iter()
                        .flat_map(|ty| {
                            self.autoderef(ty, |step| self.method_return(step, method, scalar))
                        })
                        .flatten()
                        .collect(),
                )
            }
            Desc::CallRet {
                path,
                owner: Some(owner),
                ..
            } => {
                let types = self.known(owner, Scope::from(f), &self.self_bindings(fid), 0);
                let selector = self.selector(path, fid);
                dedup(
                    types
                        .iter()
                        .flat_map(|ty| {
                            self.returned(
                                &self.called(self.qualified_method(path, ty, fid)),
                                Some(ty),
                                selector.as_deref(),
                            )
                        })
                        .collect(),
                )
            }
            Desc::CallRet {
                path,
                owner: None,
                arg,
            } => self.call_type(path, arg.as_deref(), fid, depth),
            Desc::Payload { of, variant, index } => {
                self.payload_type(of, variant, index, fid, depth)
            }
            Desc::Try(of) => self
                .rtype(of, fid, depth + 1)
                .iter()
                .filter_map(standard::tried)
                .flatten()
                .collect(),
            Desc::Index { of, index } => {
                let scalar = self.scalar_index(index, fid, depth + 1);
                self.rtype(of, fid, depth + 1)
                    .iter()
                    .filter_map(|ty| standard::indexed(ty, scalar))
                    .flatten()
                    .collect()
            }
            Desc::Item(of) => self
                .rtype(of, fid, depth + 1)
                .iter()
                .filter_map(standard::item)
                .flatten()
                .collect(),
            Desc::Closure {
                of,
                method,
                arg,
                input,
            } => self
                .rtype(of, fid, depth + 1)
                .iter()
                .flat_map(|ty| self.closure_type(ty, method, *arg, *input))
                .collect(),
            Desc::Path(path) => {
                let found = self.resolve_path(path, fid);
                if !found.found.fns.is_empty() {
                    Vec::new()
                } else if found.types.is_empty() && !found.variants.is_empty() {
                    found.owners
                } else {
                    found.types
                }
            }
            Desc::Tuple(parts) => vec![Ty::Tuple(
                parts
                    .iter()
                    .map(|part| self.rtype(part, fid, depth + 1))
                    .collect(),
            )],
            Desc::Unknown | Desc::Integer(_) => Vec::new(),
        }
    }

    fn scalar_index(&mut self, desc: &'f Desc, fid: usize, depth: usize) -> bool {
        if depth > consts::TYPE_DEPTH {
            return false;
        }
        match desc {
            Desc::Integer(suffix) => suffix.is_empty() || suffix == "usize",
            Desc::Var(id) => match self.fact(fid).and_then(|f| f.body.locals.get(*id)) {
                Some(Local::From(from)) => self.scalar_index(from, fid, depth + 1),
                _ => self
                    .rtype(desc, fid, depth)
                    .iter()
                    .any(|ty| ty.is(&["std", "primitive", "usize"])),
            },
            _ => self
                .rtype(desc, fid, depth)
                .iter()
                .any(|ty| ty.is(&["std", "primitive", "usize"])),
        }
    }

    pub(super) fn self_bindings(&self, fid: usize) -> Cow<'_, Bindings> {
        self.self_bindings
            .get(fid)
            .map_or_else(|| Cow::Owned(Bindings::new()), Cow::Borrowed)
    }

    pub(in super::super) fn site_targets(&mut self, fid: usize, site: &'f Site) -> Targets {
        if site.kind == SiteKind::Call
            && let Some(recv) = &site.recv
        {
            let found = self
                .rtype(recv, fid, 0)
                .iter()
                .map(|ty| self.qualified_method(&site.path, ty, fid))
                .fold(Found::default(), |mut combined, found| {
                    combined.dispatch |= found.dispatch;
                    combined.fns.extend(found.fns);
                    combined
                });
            return Targets {
                fns: self.called(found),
                ..Targets::default()
            };
        }
        if site.kind == SiteKind::Method {
            let name = site
                .path
                .segments
                .last()
                .map(|part| part.ident.to_string())
                .unwrap_or_default();
            let found = self.resolve_method(&name, site.recv.as_ref(), fid, 0);
            return Targets {
                fns: self.called(found),
                ..Targets::default()
            };
        }
        let found = self.resolve_path(&site.path, fid);
        let types = if found.types.is_empty() {
            &found.owners
        } else {
            &found.types
        };
        let built: Vec<ItemPath> = match site.kind {
            SiteKind::New | SiteKind::Call => types
                .iter()
                .filter_map(|ty| ty.path().map(<[String]>::to_vec))
                .collect(),
            _ => Vec::new(),
        };
        let mut fns = self.called(found.found);
        if matches!(site.kind, SiteKind::New | SiteKind::Call)
            && fns.is_empty()
            && found.variants.is_empty()
        {
            for key in &built {
                fns.extend(
                    self.by_owner
                        .get(key)
                        .and_then(|methods| methods.get("drop"))
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|fid| {
                            self.trait_keys(*fid)
                                .iter()
                                .any(|tr| tr.iter().map(String::as_str).eq(["std", "ops", "Drop"]))
                        }),
                );
            }
        }
        Targets {
            built,
            fns: dedup(fns),
            variants: found.variants,
        }
    }

    pub(in super::super) fn trait_keys(&self, fid: usize) -> &[ItemPath] {
        self.traits.get(fid).map_or(&[], Vec::as_slice)
    }

    pub(in super::super) fn trait_methods(&self) -> Vec<TraitMethod> {
        let mut methods: Vec<TraitMethod> = self
            .trait_impls
            .iter()
            .filter_map(|((tr, method), impls)| {
                let own: Vec<usize> = impls
                    .iter()
                    .copied()
                    .filter(|fid| self.fact(*fid).is_some_and(|f| !f.default))
                    .collect();
                (own.len() >= 2).then(|| TraitMethod {
                    name: format!("{}::{method}", tr.join("::")),
                    impls: own,
                })
            })
            .collect();
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        methods
    }

    pub(in super::super) fn variants(&self, fid: usize, path: &syn::Path) -> Vec<Variant> {
        let Some(f) = self.fact(fid) else {
            return Vec::new();
        };
        let mut out: Vec<Variant> = self
            .path(path, Scope::from(f), 0)
            .into_iter()
            .filter_map(|key| {
                let (name, prefix) = key.split_last()?;
                self.enum_variant(&Ty::named(prefix.to_vec(), Vec::new()), name)
            })
            .collect();
        if let Some(last) = path.segments.last()
            && path.segments.len() > 1
        {
            let name = last.ident.to_string();
            let mut prefix = path.clone();
            prefix.segments.pop();
            prefix.segments.pop_punct();
            let raw = Type::Path(syn::TypePath {
                attrs: Vec::new(),
                qself: None,
                path: prefix,
            });
            out.extend(
                self.known(&raw, Scope::from(f), &self.self_bindings(fid), 0)
                    .iter()
                    .filter_map(|ty| self.enum_variant(ty, &name)),
            );
        }
        dedup(out)
    }
}

pub(super) fn module_key(krate: &str, module: &[String]) -> ItemPath {
    std::iter::once(krate.to_owned())
        .chain(module.iter().cloned())
        .collect()
}

fn identity(place: &Place, name: &str) -> ItemPath {
    let mut key = module_key(&place.krate, &place.module);
    key.push(name.to_owned());
    key
}

pub(in super::super) fn dedup<T: Clone + Eq + Hash>(items: Vec<T>) -> Vec<T> {
    if items.len() < 2 {
        return items;
    }
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}
