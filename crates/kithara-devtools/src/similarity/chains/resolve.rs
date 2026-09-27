//! Call resolution: which workspace functions a site reaches, found from module
//! paths, `use` imports and the inferred type of a method receiver.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    hash::Hash,
    slice,
};

use super::{
    arms::TraitMethod,
    body::{Desc, Local, Site, SiteKind},
    facts::{AliasFact, EnumFact, Facts, FnFact, Import, StructFact, TraitFact},
};

mod consts {
    /// Recursion bound for module paths and re-export chains.
    pub(super) const PATH_DEPTH: usize = 4;
    /// Recursion bound for receiver type inference.
    pub(super) const TYPE_DEPTH: usize = 6;
    /// Recursion bound for alias expansion.
    pub(super) const ALIAS_DEPTH: usize = 3;
    /// `Deref` steps a member lookup walks past the type itself.
    pub(super) const DEREF_DEPTH: usize = 3;
}

#[derive(Clone, Copy)]
enum TypeDef<'f> {
    Struct(&'f StructFact),
    Enum(&'f EnumFact),
    Trait(&'f TraitFact),
    Alias(&'f AliasFact),
}

impl<'f> TypeDef<'f> {
    fn krate(self) -> &'f str {
        match self {
            Self::Struct(fact) => &fact.krate,
            Self::Enum(fact) => &fact.krate,
            Self::Trait(fact) => &fact.krate,
            Self::Alias(fact) => &fact.krate,
        }
    }
}

/// Functions a site reaches, and the enum variant it builds.
#[derive(Debug, Default)]
pub(super) struct Targets {
    pub(super) variant: Option<(String, String)>,
    pub(super) fns: Vec<usize>,
}

/// Functions a method or associated path names, and whether a trait object
/// picks among them at run time.
#[derive(Default)]
struct Found {
    fns: Vec<usize>,
    dispatch: bool,
}

impl Found {
    const fn direct(fns: Vec<usize>) -> Self {
        Self {
            fns,
            dispatch: false,
        }
    }
}

enum PathFound {
    Fns(Found),
    Ctor,
    Variant { ty: Option<String>, variant: String },
}

/// A crate and a module path inside it.
type Module<'f> = (&'f str, &'f [String]);

pub(super) struct Resolver<'f> {
    facts: &'f Facts,
    by_owner: HashMap<(&'f str, &'f str), Vec<usize>>,
    /// `Deref` targets by the crate and name of the type.
    derefs: HashMap<(&'f str, &'f str), Vec<&'f AliasFact>>,
    free_mod: HashMap<(Module<'f>, &'f str), Vec<usize>>,
    impl_traits: HashMap<&'f str, BTreeSet<&'f str>>,
    memo: HashMap<(usize, *const Desc), Option<Vec<String>>>,
    trait_impls: HashMap<(&'f str, &'f str), Vec<usize>>,
    types: HashMap<&'f str, Vec<TypeDef<'f>>>,
    uses_mod: HashMap<Module<'f>, Vec<&'f Import>>,
    crates: HashSet<&'f str>,
    modules: HashSet<Module<'f>>,
    max_dyn: usize,
}

impl<'f> Resolver<'f> {
    pub(super) fn new(facts: &'f Facts, max_dyn: usize) -> Self {
        let mut resolver = Self {
            facts,
            max_dyn,
            types: HashMap::new(),
            by_owner: HashMap::new(),
            crates: HashSet::new(),
            derefs: HashMap::new(),
            trait_impls: HashMap::new(),
            impl_traits: HashMap::new(),
            free_mod: HashMap::new(),
            modules: HashSet::new(),
            uses_mod: HashMap::new(),
            memo: HashMap::new(),
        };
        resolver.index_types();
        resolver.index_fns();
        resolver.index_uses();
        resolver
    }

    fn add_module(&mut self, krate: &'f str, module: &'f [String]) {
        let prefixes = (0..=module.len()).filter_map(|len| module.get(..len));
        self.modules.extend(prefixes.map(|prefix| (krate, prefix)));
    }

    /// What `lookup` finds on `ty`, or else on the first level of `Deref`
    /// targets where it finds anything: the compiler's autoderef order.
    fn autoderef<T>(&self, ty: &str, f: &FnFact, lookup: impl Fn(&str) -> Option<T>) -> Vec<T> {
        let mut level = vec![ty.to_string()];
        let mut seen = level.clone();
        for _ in 0..=consts::DEREF_DEPTH {
            let hits: Vec<T> = level.iter().filter_map(|step| lookup(step)).collect();
            if !hits.is_empty() {
                return hits;
            }
            level = level
                .iter()
                .flat_map(|step| self.deref_targets(step, f))
                .filter(|target| !seen.contains(target))
                .collect();
            seen.extend(level.iter().cloned());
        }
        Vec::new()
    }

    /// A constructor call names its type; a std wrapper (`Arc::new(x)`,
    /// `Some(x)`) holds its argument's type.
    fn call_type(
        &mut self,
        path: &[String],
        arg: Option<&'f Desc>,
        fid: usize,
        depth: usize,
    ) -> Vec<String> {
        let (Some(f), Some((name, quals))) = (self.fact(fid), path.split_last()) else {
            return Vec::new();
        };
        let named = match quals.last() {
            Some(qualifier) if is_upper(qualifier) => own(f, qualifier).map(|ty| {
                let returned = self.returns(&self.methods_of(ty, name, &f.place.krate).fns);
                (ty, returned)
            }),
            _ if is_upper(name) => own(f, name).map(|ty| (ty, Vec::new())),
            _ => {
                return match self.resolve_path(path, fid) {
                    PathFound::Fns(found) => self.returns(&found.fns),
                    PathFound::Ctor | PathFound::Variant { .. } => Vec::new(),
                };
            }
        };
        match named {
            Some((_, returned)) if !returned.is_empty() => returned,
            Some((ty, _)) if self.is_type(ty) => vec![ty.to_string()],
            _ => arg
                .map(|arg| self.rtype(arg, fid, depth + 1))
                .unwrap_or_default(),
        }
    }

    /// A trait-object call with more impls than the bound is a hub, not a path.
    fn called(&self, found: Found) -> Targets {
        let hub = found.dispatch && found.fns.len() > self.max_dyn;
        Targets {
            fns: if hub { Vec::new() } else { found.fns },
            variant: None,
        }
    }

    /// Workspace types the `Deref` impls of `ty` target, for each definition
    /// of `ty` the crate of `f` sees.
    fn deref_targets(&self, ty: &str, f: &FnFact) -> Vec<String> {
        let mut out = Vec::new();
        for def in self.type_defs(ty, &f.place.krate) {
            for target in self.derefs.get(&(def.krate(), ty)).into_iter().flatten() {
                out.extend(self.known(&target.ty, f, 0));
            }
        }
        dedup(out)
    }

    /// Building a value of a type with a `Drop` impl schedules that impl.
    fn drop_of(&self, ty: Option<&str>) -> Vec<usize> {
        let Some(ty) = ty else {
            return Vec::new();
        };
        let impls = self
            .by_owner
            .get(&(ty, "drop"))
            .into_iter()
            .flatten()
            .copied();
        impls
            .filter(|id| {
                self.fact(*id)
                    .is_some_and(|f| f.trait_name.as_deref() == Some("Drop"))
            })
            .collect()
    }

    fn enum_variant(
        &self,
        ty: Option<&str>,
        variant: &str,
        krate: &str,
    ) -> Option<(String, String)> {
        let ty = ty?;
        self.type_defs(ty, krate)
            .into_iter()
            .any(|def| matches!(def, TypeDef::Enum(data) if data.variants.iter().any(|v| v == variant)))
            .then(|| (ty.to_string(), variant.to_string()))
    }

    fn fact(&self, id: usize) -> Option<&'f FnFact> {
        let facts = self.facts;
        facts.fns.get(id)
    }

    /// A struct field typed by the struct's own generic parameter takes the
    /// impl's argument, or else the parameter's bounds.
    fn field_idents(data: &StructFact, idents: &[String], ty: &str, f: &FnFact) -> Vec<String> {
        let mut out = Vec::new();
        for ident in idents {
            let Some(index) = data.params.iter().position(|param| param == ident) else {
                out.push(ident.clone());
                continue;
            };
            match f.self_args.get(index) {
                Some(args) if f.owner.as_deref() == Some(ty) => out.extend(args.iter().cloned()),
                _ => out.extend(data.bounds.get(ident).into_iter().flatten().cloned()),
            }
        }
        out
    }

    /// Workspace types of `field` on `ty`, when a struct named `ty` declares
    /// the field.
    fn field_type(&self, ty: &str, field: &str, f: &FnFact) -> Option<Vec<String>> {
        let mut declared: Option<Vec<String>> = None;
        for def in self.type_defs(ty, &f.place.krate) {
            if let TypeDef::Struct(data) = def
                && let Some(idents) = data.fields.get(field)
            {
                let known = self.known(&Self::field_idents(data, idents, ty, f), f, 0);
                declared.get_or_insert_default().extend(known);
            }
        }
        declared
    }

    fn in_crate(&self, id: usize, krate: &str) -> bool {
        self.fact(id).is_some_and(|f| f.place.krate == krate)
    }

    fn index_fns(&mut self) {
        let facts = self.facts;
        for (id, f) in facts.fns.iter().enumerate() {
            let name = f.name.as_str();
            if let Some(owner) = f.owner.as_deref() {
                self.by_owner.entry((owner, name)).or_default().push(id);
                if let Some(tr) = f.trait_name.as_deref()
                    && tr != owner
                {
                    self.impl_traits.entry(owner).or_default().insert(tr);
                }
            } else {
                let key = ((f.place.krate.as_str(), f.place.module.as_slice()), name);
                self.free_mod.entry(key).or_default().push(id);
            }
            if let Some(tr) = f.trait_name.as_deref() {
                self.trait_impls.entry((tr, name)).or_default().push(id);
            }
            self.add_module(&f.place.krate, &f.place.module);
            self.crates.insert(&f.place.krate);
        }
    }

    fn index_types(&mut self) {
        let facts = self.facts;
        let defs = facts
            .structs
            .iter()
            .map(|f| (f.name.as_str(), TypeDef::Struct(f)))
            .chain(
                facts
                    .enums
                    .iter()
                    .map(|f| (f.name.as_str(), TypeDef::Enum(f))),
            )
            .chain(
                facts
                    .traits
                    .iter()
                    .map(|f| (f.name.as_str(), TypeDef::Trait(f))),
            )
            .chain(
                facts
                    .aliases
                    .iter()
                    .map(|f| (f.name.as_str(), TypeDef::Alias(f))),
            );
        for (name, def) in defs {
            self.types.entry(name).or_default().push(def);
        }
        for fact in &facts.derefs {
            self.derefs
                .entry((fact.krate.as_str(), fact.name.as_str()))
                .or_default()
                .push(fact);
        }
    }

    fn index_uses(&mut self) {
        let facts = self.facts;
        for fact in &facts.uses {
            self.uses_mod
                .entry((fact.krate.as_str(), fact.module.as_slice()))
                .or_default()
                .push(&fact.import);
            self.add_module(&fact.krate, &fact.module);
        }
    }

    fn is_trait(&self, name: &str, krate: &str) -> bool {
        matches!(self.type_defs(name, krate).first(), Some(TypeDef::Trait(_)))
    }

    fn is_type(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    /// Every workspace type named in a type expression, outermost first:
    /// `Mutex<Inner>` gives both.
    fn known(&self, idents: &[String], f: &FnFact, depth: usize) -> Vec<String> {
        let mut out = Vec::new();
        for ident in idents {
            if let Some(bounds) = f.generics.get(ident) {
                out.extend(bounds.iter().filter(|bound| self.is_type(bound)).cloned());
            } else if ident == "Self" {
                out.extend(f.owner.iter().cloned());
            } else if self.is_type(ident) {
                let defs = self.type_defs(ident, &f.place.krate);
                if !matches!(defs.first(), Some(TypeDef::Alias(_))) {
                    out.push(ident.clone());
                } else if depth < consts::ALIAS_DEPTH {
                    for def in defs {
                        if let TypeDef::Alias(alias) = def {
                            out.extend(self.known(&alias.ty, f, depth + 1));
                        }
                    }
                }
            }
        }
        dedup(out)
    }

    /// Functions at an absolute path: an associated function, a free function
    /// of the module, or one the module imports by name or by glob.
    fn lookup_abs(
        &self,
        krate: &str,
        abs: &[String],
        local: &[Import],
        depth: usize,
    ) -> Vec<usize> {
        let Some((name, module)) = abs.split_last() else {
            return Vec::new();
        };
        if depth > consts::PATH_DEPTH {
            return Vec::new();
        }
        if let Some(owner) = module.last()
            && is_upper(owner)
        {
            return self.methods_of(owner, name, krate).fns;
        }
        if let Some(found) = self.free_mod.get(&((krate, module), name.as_str()))
            && !found.is_empty()
        {
            return found.clone();
        }
        let scoped = self
            .uses_mod
            .get(&(krate, module))
            .into_iter()
            .flatten()
            .copied();
        for import in local.iter().chain(scoped) {
            let path = match import.alias.as_str() {
                "*" => [import.path.as_slice(), slice::from_ref(name)].concat(),
                alias if alias == name => import.path.clone(),
                _ => continue,
            };
            let Some((target, resolved)) = self.norm_path(krate, module, &path, local, depth + 1)
            else {
                continue;
            };
            if target == krate && resolved == abs {
                continue;
            }
            let found = self.lookup_abs(&target, &resolved, &[], depth + 1);
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    fn methods_of(&self, ty: &str, method: &str, krate: &str) -> Found {
        if self.is_trait(ty, krate) {
            return Found {
                fns: self
                    .trait_impls
                    .get(&(ty, method))
                    .cloned()
                    .unwrap_or_default(),
                dispatch: true,
            };
        }
        let mut fns = self
            .by_owner
            .get(&(ty, method))
            .cloned()
            .unwrap_or_default();
        if fns.is_empty() {
            for tr in self.impl_traits.get(ty).into_iter().flatten() {
                let defaults = self.by_owner.get(&(*tr, method)).into_iter().flatten();
                fns.extend(defaults.filter(|id| self.fact(**id).is_some_and(|f| f.default)));
            }
        }
        Found::direct(self.prefer_crate(fns, krate))
    }

    /// Crate and absolute item path of a path written inside `module`.
    fn norm_path(
        &self,
        krate: &str,
        module: &[String],
        segs: &[String],
        local: &[Import],
        depth: usize,
    ) -> Option<(String, Vec<String>)> {
        let (head, rest) = segs.split_first()?;
        if depth > consts::PATH_DEPTH {
            return None;
        }
        match head.as_str() {
            "crate" => return Some((krate.to_string(), rest.to_vec())),
            "self" => return Some((krate.to_string(), [module, rest].concat())),
            "super" => {
                let supers = segs.iter().take_while(|seg| *seg == "super").count();
                let parent = module
                    .get(..module.len().saturating_sub(supers))
                    .unwrap_or_default();
                let tail = segs.get(supers..).unwrap_or_default();
                return Some((krate.to_string(), [parent, tail].concat()));
            }
            _ => {}
        }
        if let Some(target) = self.workspace_crate(head) {
            return Some((target, rest.to_vec()));
        }
        let scoped = self
            .uses_mod
            .get(&(krate, module))
            .into_iter()
            .flatten()
            .copied();
        for import in local.iter().chain(scoped) {
            if import.alias == *head
                && let Some((target, path)) =
                    self.norm_path(krate, module, &import.path, &[], depth + 1)
            {
                return Some((target, [path.as_slice(), rest].concat()));
            }
        }
        let child = [module, slice::from_ref(head)].concat();
        self.modules
            .contains(&(krate, child.as_slice()))
            .then(|| (krate.to_string(), [module, segs].concat()))
    }

    /// `Struct { field }` names the struct itself; `Enum::Variant(x)` or
    /// `Variant(x)` names a variant whose enum the scrutinee's type gives. A std
    /// enum (`Some`, `Ok`) carries the scrutinee's own type arguments.
    fn payload_type(
        &mut self,
        of: &'f Desc,
        variant: &[String],
        index: &str,
        fid: usize,
        depth: usize,
    ) -> Vec<String> {
        let (Some(f), Some((last, quals))) = (self.fact(fid), variant.split_last()) else {
            return Vec::new();
        };
        let named = own(f, last);
        let mut owners: Vec<String> = named.iter().map(|name| (*name).to_string()).collect();
        match quals.last() {
            Some(enum_name) => owners.extend(own(f, enum_name).map(str::to_string)),
            None => owners.extend(self.rtype(of, fid, depth + 1)),
        }
        let mut out = Vec::new();
        for ty in dedup(owners) {
            for def in self.type_defs(&ty, &f.place.krate) {
                match def {
                    TypeDef::Enum(data) => {
                        if let Some(idents) =
                            data.payload.get(last).and_then(|fields| fields.get(index))
                        {
                            out.extend(self.known(idents, f, 0));
                        }
                    }
                    TypeDef::Struct(data) if named == Some(ty.as_str()) => {
                        if let Some(idents) = data.fields.get(index) {
                            out.extend(self.known(&Self::field_idents(data, idents, &ty, f), f, 0));
                        }
                    }
                    TypeDef::Struct(_) | TypeDef::Trait(_) | TypeDef::Alias(_) => {}
                }
            }
        }
        if out.is_empty() {
            return self.rtype(of, fid, depth + 1);
        }
        dedup(out)
    }

    fn prefer_crate(&self, ids: Vec<usize>, krate: &str) -> Vec<usize> {
        let same: Vec<usize> = ids
            .iter()
            .copied()
            .filter(|id| self.in_crate(*id, krate))
            .collect();
        if same.is_empty() { ids } else { same }
    }

    /// Every type of the receiver that has the method: `Mutex<Inner>` reaches
    /// `Inner` through the guard, a per-cfg alias reaches each target, and a
    /// type without the method reaches its `Deref` target.
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
        let Some(f) = self.fact(fid) else {
            return Found::default();
        };
        let krate = f.place.krate.as_str();
        let mut found = Found::default();
        for ty in &types {
            let hits = self.autoderef(ty, f, |step| {
                let hit = self.methods_of(step, method, krate);
                (!hit.fns.is_empty()).then_some(hit)
            });
            for Found { fns, dispatch } in hits {
                found.dispatch |= dispatch;
                found.fns.extend(fns);
            }
        }
        found.fns = dedup(found.fns);
        found
    }

    fn resolve_path(&self, path: &[String], fid: usize) -> PathFound {
        let (Some(f), Some((name, quals))) = (self.fact(fid), path.split_last()) else {
            return PathFound::Fns(Found::default());
        };
        let krate = f.place.krate.as_str();
        let module = f.place.module.as_slice();
        let local = f.body.uses.as_slice();
        let scoped = match quals.last() {
            None if !is_upper(name) => {
                let abs = [module, slice::from_ref(name)].concat();
                self.lookup_abs(krate, &abs, local, 0)
            }
            Some(qualifier) if !is_upper(qualifier) => self
                .norm_path(krate, module, path, local, 0)
                .map(|(target, abs)| self.lookup_abs(&target, &abs, &[], 0))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if !scoped.is_empty() {
            return PathFound::Fns(Found::direct(scoped));
        }
        match quals.last() {
            None if is_upper(name) => PathFound::Ctor,
            Some(qualifier) if is_upper(qualifier) => {
                let ty = own(f, qualifier);
                if is_upper(name) {
                    return PathFound::Variant {
                        ty: ty.map(str::to_string),
                        variant: name.clone(),
                    };
                }
                PathFound::Fns(
                    ty.map_or_else(Found::default, |ty| self.methods_of(ty, name, krate)),
                )
            }
            None | Some(_) => PathFound::Fns(Found::default()),
        }
    }

    fn returns(&self, fns: &[usize]) -> Vec<String> {
        let out = fns
            .iter()
            .filter_map(|id| self.fact(*id))
            .flat_map(|g| self.known(&g.ret, g, 0))
            .collect();
        dedup(out)
    }

    /// Workspace types of an expression. A cycle through the same expression
    /// resolves to nothing.
    fn rtype(&mut self, desc: &'f Desc, fid: usize, depth: usize) -> Vec<String> {
        if depth > consts::TYPE_DEPTH {
            return Vec::new();
        }
        let key = (fid, std::ptr::from_ref(desc));
        if let Some(known) = self.memo.get(&key) {
            return known.clone().unwrap_or_default();
        }
        self.memo.insert(key, None);
        let found = self.rtype_of(desc, fid, depth);
        self.memo.insert(key, Some(found.clone()));
        found
    }

    fn rtype_of(&mut self, desc: &'f Desc, fid: usize, depth: usize) -> Vec<String> {
        let Some(f) = self.fact(fid) else {
            return Vec::new();
        };
        match desc {
            Desc::SelfValue => f.owner.iter().cloned().collect(),
            Desc::Var(id) => match f.body.locals.get(*id) {
                Some(Local::Ty(ty)) => self.known(ty, f, 0),
                Some(Local::From(from)) => self.rtype(from, fid, depth + 1),
                None => Vec::new(),
            },
            Desc::Field { of, field } => {
                let mut out = Vec::new();
                for ty in self.rtype(of, fid, depth + 1) {
                    let hits = self.autoderef(&ty, f, |step| self.field_type(step, field, f));
                    out.extend(hits.into_iter().flatten());
                }
                dedup(out)
            }
            Desc::Payload { of, variant, index } => {
                self.payload_type(of, variant, index, fid, depth)
            }
            Desc::Ret { of, method } => {
                let fns = self.resolve_method(method, Some(of), fid, depth + 1).fns;
                self.returns(&fns)
            }
            Desc::CallRet { path, arg } => self.call_type(path, arg.as_deref(), fid, depth),
            Desc::Ty(ty) => self.known(ty, f, 0),
            Desc::Path(path) => match self.resolve_path(path, fid) {
                PathFound::Variant { ty: Some(ty), .. } => vec![ty],
                PathFound::Fns(_) | PathFound::Ctor | PathFound::Variant { .. } => Vec::new(),
            },
            Desc::Tuple(_) | Desc::Unknown => Vec::new(),
        }
    }

    pub(super) fn site_targets(&mut self, fid: usize, site: &'f Site) -> Targets {
        let Some(f) = self.fact(fid) else {
            return Targets::default();
        };
        let krate = f.place.krate.as_str();
        let built = || site.path.last().and_then(|name| own(f, name));
        match site.kind {
            SiteKind::Method => {
                let method = site.path.first().map_or("", String::as_str);
                let found = self.resolve_method(method, site.recv.as_ref(), fid, 0);
                self.called(found)
            }
            SiteKind::Call | SiteKind::Ref => match self.resolve_path(&site.path, fid) {
                PathFound::Fns(found) => self.called(found),
                PathFound::Ctor => Targets {
                    fns: self.drop_of(built()),
                    variant: None,
                },
                PathFound::Variant { ty, variant } => Targets {
                    fns: Vec::new(),
                    variant: self.enum_variant(ty.as_deref(), &variant, krate),
                },
            },
            SiteKind::New => Targets {
                fns: self.drop_of(built()),
                variant: None,
            },
            SiteKind::Variant => Targets {
                fns: Vec::new(),
                variant: self.variant(fid, &site.path),
            },
        }
    }

    /// Methods of a workspace trait with two or more impls of their own, in
    /// name order.
    pub(super) fn trait_methods(&self) -> Vec<TraitMethod> {
        let mut methods: Vec<TraitMethod> = self
            .trait_impls
            .iter()
            .filter_map(|(&(tr, method), impls)| {
                let own: Vec<usize> = impls
                    .iter()
                    .copied()
                    .filter(|&id| {
                        self.fact(id)
                            .is_some_and(|f| !f.default && f.owner.as_deref() != Some(tr))
                    })
                    .collect();
                let first = self.fact(*own.first()?)?;
                (own.len() >= 2 && self.is_trait(tr, &first.place.krate)).then(|| TraitMethod {
                    name: format!("{tr}::{method}"),
                    impls: own,
                })
            })
            .collect();
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        methods
    }

    fn type_defs(&self, name: &str, krate: &str) -> Vec<TypeDef<'f>> {
        let Some(defs) = self.types.get(name) else {
            return Vec::new();
        };
        let same: Vec<TypeDef<'f>> = defs
            .iter()
            .copied()
            .filter(|d| d.krate() == krate)
            .collect();
        if same.is_empty() { defs.clone() } else { same }
    }

    /// The workspace enum variant a qualified path names inside `fid`.
    pub(super) fn variant(&self, fid: usize, path: &[String]) -> Option<(String, String)> {
        let f = self.fact(fid)?;
        let [.., ty, variant] = path else {
            return None;
        };
        self.enum_variant(own(f, ty), variant, &f.place.krate)
    }

    /// The scanned crate an extern path head names: `kithara_hls` is
    /// `kithara-hls`.
    fn workspace_crate(&self, head: &str) -> Option<String> {
        let name = head.replace('_', "-");
        self.crates.contains(name.as_str()).then_some(name)
    }
}

/// The type a name denotes inside `f`: `Self` is the impl's owner.
pub(super) fn own<'a>(f: &'a FnFact, name: &'a str) -> Option<&'a str> {
    if name == "Self" {
        f.owner.as_deref()
    } else {
        Some(name)
    }
}

pub(super) fn is_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// Removes repeats, keeping the first occurrence of each.
pub(super) fn dedup<T: Clone + Eq + Hash>(items: Vec<T>) -> Vec<T> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}
