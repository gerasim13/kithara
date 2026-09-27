//! Item facts of the chain detector: functions with their bodies, type
//! definitions, aliases and `use` imports, placed by crate and module path.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{
    Attribute, Expr, FnArg, GenericArgument, GenericParam, ImplItem, Item, Lit, Meta, Pat,
    PathArguments, ReturnType, TraitItem, Type, TypeParamBound, UseTree, WherePredicate,
    parse::{ParseStream, Parser},
    spanned::Spanned,
    visit,
    visit::Visit,
};

use super::body::{Body, BodyFacts, norm_tokens};

mod consts {
    /// Attributes that hold no code a derive expands into the type's impl.
    pub(super) const PLAIN_ATTRIBUTES: &[&str] = &[
        "doc",
        "cfg",
        "cfg_attr",
        "derive",
        "allow",
        "expect",
        "warn",
        "deny",
        "repr",
        "must_use",
        "non_exhaustive",
        "serde",
    ];
    /// Name and trait of the synthetic function that holds a derive's calls.
    pub(super) const DERIVE_FN: &str = "#derive";
}

/// A `use` binding: the name it brings into scope and the path it names. A
/// glob import has the alias `*`.
#[derive(Clone, Debug)]
pub(super) struct Import {
    pub(super) alias: String,
    pub(super) path: Vec<String>,
}

#[derive(Debug)]
pub(super) struct UseFact {
    pub(super) import: Import,
    pub(super) krate: String,
    pub(super) module: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Place {
    pub(super) file: String,
    pub(super) krate: String,
    pub(super) module: Vec<String>,
    pub(super) end: usize,
    pub(super) line: usize,
}

#[derive(Debug)]
pub(super) struct FnFact {
    /// Bounds of every generic parameter in scope, impl parameters included.
    pub(super) generics: BTreeMap<String, Vec<String>>,
    pub(super) body: BodyFacts,
    pub(super) owner: Option<String>,
    pub(super) trait_name: Option<String>,
    pub(super) place: Place,
    pub(super) name: String,
    pub(super) cfg: Vec<String>,
    pub(super) ret: Vec<String>,
    /// Type arguments of the impl's self type: `impl Foo<Bar>` holds `[[Bar]]`.
    pub(super) self_args: Vec<Vec<String>>,
    pub(super) tokens: Vec<String>,
    pub(super) default: bool,
    pub(super) public: bool,
}

#[derive(Debug)]
pub(super) struct StructFact {
    pub(super) bounds: BTreeMap<String, Vec<String>>,
    pub(super) fields: BTreeMap<String, Vec<String>>,
    pub(super) krate: String,
    pub(super) name: String,
    pub(super) params: Vec<String>,
}

#[derive(Debug)]
pub(super) struct EnumFact {
    pub(super) payload: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub(super) krate: String,
    pub(super) name: String,
    pub(super) variants: Vec<String>,
}

#[derive(Debug)]
pub(super) struct TraitFact {
    pub(super) krate: String,
    pub(super) name: String,
}

#[derive(Debug)]
pub(super) struct AliasFact {
    pub(super) krate: String,
    pub(super) name: String,
    pub(super) ty: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct Facts {
    pub(super) aliases: Vec<AliasFact>,
    pub(super) enums: Vec<EnumFact>,
    pub(super) fns: Vec<FnFact>,
    pub(super) structs: Vec<StructFact>,
    pub(super) traits: Vec<TraitFact>,
    pub(super) uses: Vec<UseFact>,
}

/// The file that declares a module with `mod name;`.
struct Parent {
    file: String,
    name: String,
    cfg: Vec<String>,
}

/// Parses every source and records its items; test modules and items are left
/// out.
pub(super) fn collect(sources: &[(String, String)]) -> Result<Facts> {
    let mut parsed = BTreeMap::new();
    for (path, text) in sources {
        let file = syn::parse_file(text)
            .with_context(|| format!("parse Rust source for chains: {path}"))?;
        parsed.insert(path.as_str(), file);
    }
    let parents = module_parents(&parsed);
    let mut facts = Facts::default();
    for (path, file) in &parsed {
        let Some(krate) = crate_of(path) else {
            continue;
        };
        let (cfg, module) = ancestry(path, &parents);
        if cfg.iter().any(|c| is_test_cfg(c)) {
            continue;
        }
        let mut visitor = ItemVisitor {
            krate,
            cfg,
            module,
            facts: &mut facts,
            file: (*path).to_string(),
            scope: ImplScope::default(),
        };
        visitor.visit_file(file);
    }
    Ok(facts)
}

/// The crate is the directory above the first `src` component.
fn crate_of(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let src = parts.iter().position(|part| *part == "src")?;
    parts
        .get(src.checked_sub(1)?)
        .map(|name| (*name).to_string())
}

fn module_parents(parsed: &BTreeMap<&str, syn::File>) -> BTreeMap<String, Parent> {
    let mut parents = BTreeMap::new();
    for (path, file) in parsed {
        let dir = mod_dir(path);
        for item in &file.items {
            let Item::Mod(module) = item else {
                continue;
            };
            if module.content.is_some() {
                continue;
            }
            let name = module.ident.to_string();
            let candidates = path_attr(&module.attrs).map_or_else(
                || vec![format!("{dir}/{name}.rs"), format!("{dir}/{name}/mod.rs")],
                |target| vec![format!("{}/{target}", parent_dir(path))],
            );
            for candidate in candidates {
                if parsed.contains_key(candidate.as_str()) {
                    parents.insert(
                        candidate,
                        Parent {
                            file: (*path).to_string(),
                            cfg: cfgs_of(&module.attrs),
                            name: name.clone(),
                        },
                    );
                }
            }
        }
    }
    parents
}

/// The cfgs a file inherits from its `mod` declarations and its module path.
fn ancestry(path: &str, parents: &BTreeMap<String, Parent>) -> (Vec<String>, Vec<String>) {
    let mut cfg = Vec::new();
    let mut module = Vec::new();
    let mut current = path;
    while let Some(parent) = parents.get(current) {
        cfg.extend(parent.cfg.iter().cloned());
        module.insert(0, parent.name.clone());
        current = &parent.file;
    }
    (cfg, module)
}

fn path_attr(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        let Meta::NameValue(value) = &attr.meta else {
            return None;
        };
        let Expr::Lit(literal) = &value.value else {
            return None;
        };
        let Lit::Str(text) = &literal.lit else {
            return None;
        };
        Some(text.value())
    })
}

fn parent_dir(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn mod_dir(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if matches!(stem.as_str(), "mod" | "lib" | "main") {
        parent_dir(path)
    } else {
        format!("{}/{stem}", parent_dir(path))
    }
}

pub(super) fn cfgs_of(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .filter_map(|attr| match &attr.meta {
            Meta::List(list) => Some(list.tokens.to_string()),
            Meta::Path(_) | Meta::NameValue(_) => None,
        })
        .collect()
}

/// `test` alone or as a top-level term of `all(..)`.
fn is_test_cfg(cfg: &str) -> bool {
    let cfg = cfg.replace(' ', "");
    if cfg == "test" {
        return true;
    }
    let Some(inner) = cfg
        .strip_prefix("all(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, ch) in inner.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                if inner.get(start..index) == Some("test") {
                    return true;
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    inner.get(start..) == Some("test")
}

fn is_test_item(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|s| matches!(s.ident.to_string().as_str(), "test" | "rstest" | "bench"))
    }) || cfgs_of(attrs).iter().any(|c| is_test_cfg(c))
}

pub(super) fn path_segs(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

fn type_idents(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Path(path) => {
            if let Some(qself) = &path.qself {
                type_idents(&qself.ty, out);
            }
            for segment in &path.path.segments {
                out.push(segment.ident.to_string());
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let GenericArgument::Type(ty) = arg {
                            type_idents(ty, out);
                        }
                    }
                }
            }
        }
        Type::Reference(reference) => type_idents(&reference.elem, out),
        Type::Slice(slice) => type_idents(&slice.elem, out),
        Type::Array(array) => type_idents(&array.elem, out),
        Type::Ptr(ptr) => type_idents(&ptr.elem, out),
        Type::Paren(paren) => type_idents(&paren.elem, out),
        Type::Group(group) => type_idents(&group.elem, out),
        Type::Tuple(tuple) => {
            for elem in &tuple.elems {
                type_idents(elem, out);
            }
        }
        Type::TraitObject(object) => bounds_idents(object.bounds.iter(), out),
        Type::ImplTrait(object) => bounds_idents(object.bounds.iter(), out),
        _ => {}
    }
}

fn bounds_idents<'a>(bounds: impl Iterator<Item = &'a TypeParamBound>, out: &mut Vec<String>) {
    for bound in bounds {
        if let TypeParamBound::Trait(tr) = bound
            && let Some(segment) = tr.path.segments.last()
        {
            out.push(segment.ident.to_string());
        }
    }
}

/// Every identifier named in a type, outermost first: `Mutex<Inner>` gives
/// `[Mutex, Inner]`.
pub(super) fn idents_of(ty: &Type) -> Vec<String> {
    let mut out = Vec::new();
    type_idents(ty, &mut out);
    out
}

pub(super) fn last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(reference) => last_ident(&reference.elem),
        _ => None,
    }
}

fn generics_map(generics: &syn::Generics, into: &mut BTreeMap<String, Vec<String>>) {
    for param in &generics.params {
        if let GenericParam::Type(ty) = param {
            bounds_idents(
                ty.bounds.iter(),
                into.entry(ty.ident.to_string()).or_default(),
            );
        }
    }
    let Some(clause) = &generics.where_clause else {
        return;
    };
    for predicate in &clause.predicates {
        if let WherePredicate::Type(bound) = predicate
            && let Type::Path(path) = &bound.bounded_ty
            && path.path.segments.len() == 1
            && let Some(segment) = path.path.segments.first()
        {
            bounds_idents(
                bound.bounds.iter(),
                into.entry(segment.ident.to_string()).or_default(),
            );
        }
    }
}

/// Flattens a `use` tree into its bindings.
pub(super) fn use_tree(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<Import>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            use_tree(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let name = name.ident.to_string();
            let mut path = prefix.clone();
            let alias = if name == "self" {
                prefix.last().cloned().unwrap_or_default()
            } else {
                path.push(name.clone());
                name
            };
            out.push(Import { alias, path });
        }
        UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            if rename.ident != "self" {
                path.push(rename.ident.to_string());
            }
            out.push(Import {
                path,
                alias: rename.rename.to_string(),
            });
        }
        UseTree::Glob(_) => out.push(Import {
            alias: "*".to_string(),
            path: prefix.clone(),
        }),
        UseTree::Group(group) => {
            for item in &group.items {
                use_tree(item, prefix, out);
            }
        }
    }
}

fn self_type_args(ty: &Type) -> Vec<Vec<String>> {
    let Type::Path(path) = ty else {
        return Vec::new();
    };
    let Some(segment) = path.path.segments.last() else {
        return Vec::new();
    };
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Vec::new();
    };
    args.args
        .iter()
        .filter_map(|arg| match arg {
            GenericArgument::Type(ty) => Some(idents_of(ty)),
            _ => None,
        })
        .collect()
}

type DelegateItem = (Vec<Attribute>, bool, syn::Signature);

fn parse_delegate_items(input: ParseStream<'_>) -> syn::Result<Vec<DelegateItem>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: syn::Visibility = input.parse()?;
        let sig: syn::Signature = input.parse()?;
        if input.peek(syn::Token![;]) {
            input.parse::<syn::Token![;]>()?;
        } else {
            input.parse::<syn::Block>()?;
        }
        out.push((attrs, matches!(vis, syn::Visibility::Public(_)), sig));
    }
    Ok(out)
}

/// The impl or trait an item visitor is inside.
#[derive(Default)]
struct ImplScope {
    generics: BTreeMap<String, Vec<String>>,
    owner: Option<String>,
    trait_name: Option<String>,
    self_args: Vec<Vec<String>>,
}

struct ItemVisitor<'f> {
    facts: &'f mut Facts,
    scope: ImplScope,
    file: String,
    krate: String,
    cfg: Vec<String>,
    module: Vec<String>,
}

impl ItemVisitor<'_> {
    /// Derive helper attributes (`#[painter(draw = self.paint(..))]`) hold code
    /// the derive expands into an impl of the type: their calls belong to a
    /// synthetic function of that type.
    fn add_attr_calls(&mut self, owner: &syn::Ident, attrs: &[Attribute]) {
        let mut body = Body::default();
        body.bind_receiver();
        for attr in attrs {
            if consts::PLAIN_ATTRIBUTES
                .iter()
                .any(|plain| attr.path().is_ident(plain))
            {
                continue;
            }
            if let Meta::List(list) = &attr.meta {
                body.scan_tokens(list.tokens.clone(), attr.span().start().line);
            }
        }
        if !body.has_sites() {
            return;
        }
        let line = owner.span().start().line;
        let fact = FnFact {
            place: self.place(line, line),
            name: consts::DERIVE_FN.to_string(),
            owner: Some(owner.to_string()),
            trait_name: Some(consts::DERIVE_FN.to_string()),
            public: true,
            default: false,
            cfg: self.cfg.clone(),
            generics: BTreeMap::new(),
            self_args: Vec::new(),
            ret: Vec::new(),
            tokens: Vec::new(),
            body: body.finish(),
        };
        self.facts.fns.push(fact);
    }

    /// `delegate! { to <target> { fn name(..); } }` forwards each listed
    /// method to `<target>`.
    fn add_delegate(&mut self, mac: &syn::Macro) {
        let trees: Vec<TokenTree> = mac.tokens.clone().into_iter().collect();
        let mut index = 0;
        while let Some(tree) = trees.get(index) {
            index += 1;
            if !matches!(tree, TokenTree::Ident(kw) if kw == "to") {
                continue;
            }
            let mut target = TokenStream::new();
            while let Some(tree) = trees.get(index) {
                if matches!(tree, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace) {
                    break;
                }
                target.extend([tree.clone()]);
                index += 1;
            }
            let Some(TokenTree::Group(body)) = trees.get(index) else {
                break;
            };
            index += 1;
            let Ok(items) = Parser::parse2(parse_delegate_items, body.stream()) else {
                continue;
            };
            for (attrs, public, sig) in items {
                self.add_delegated(&target, attrs, public, &sig);
            }
        }
    }

    fn add_delegated(
        &mut self,
        target: &TokenStream,
        attrs: Vec<Attribute>,
        public: bool,
        sig: &syn::Signature,
    ) {
        let call = attrs
            .iter()
            .find(|attr| attr.path().is_ident("call"))
            .and_then(|attr| attr.parse_args::<syn::Ident>().ok())
            .unwrap_or_else(|| sig.ident.clone());
        let args: Vec<&syn::Ident> = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                FnArg::Typed(typed) => match typed.pat.as_ref() {
                    Pat::Ident(ident) => Some(&ident.ident),
                    _ => None,
                },
                FnArg::Receiver(_) => None,
            })
            .collect();
        let Ok(block) =
            syn::parse2::<syn::Block>(quote::quote! { { #target . #call ( #(#args),* ) } })
        else {
            return;
        };
        let keep: Vec<Attribute> = attrs
            .into_iter()
            .filter(|attr| attr.path().is_ident("cfg"))
            .collect();
        self.add_fn(&keep, sig, &block, public, false);
    }

    fn add_fn(
        &mut self,
        attrs: &[Attribute],
        sig: &syn::Signature,
        block: &syn::Block,
        public: bool,
        default: bool,
    ) {
        if is_test_item(attrs) {
            return;
        }
        let mut generics = self.scope.generics.clone();
        generics_map(&sig.generics, &mut generics);
        let mut body = Body::default();
        for input in &sig.inputs {
            match input {
                FnArg::Receiver(_) => body.bind_receiver(),
                FnArg::Typed(typed) => {
                    if let Pat::Ident(ident) = typed.pat.as_ref() {
                        body.bind_param(ident.ident.to_string(), idents_of(&typed.ty));
                    }
                }
            }
        }
        body.visit_block(block);
        let line = sig.ident.span().start().line;
        let end = block.span().end().line.max(line);
        let ret = match &sig.output {
            ReturnType::Default => Vec::new(),
            ReturnType::Type(_, ty) => idents_of(ty),
        };
        let mut cfg = self.cfg.clone();
        cfg.extend(cfgs_of(attrs));
        let fact = FnFact {
            public,
            default,
            cfg,
            generics,
            ret,
            place: self.place(line, end),
            name: sig.ident.to_string(),
            owner: self.scope.owner.clone(),
            trait_name: self.scope.trait_name.clone(),
            self_args: self.scope.self_args.clone(),
            tokens: norm_tokens(block.to_token_stream()),
            body: body.finish(),
        };
        self.facts.fns.push(fact);
    }

    fn place(&self, line: usize, end: usize) -> Place {
        Place {
            line,
            end,
            krate: self.krate.clone(),
            file: self.file.clone(),
            module: self.module.clone(),
        }
    }

    fn with_cfg(&mut self, attrs: &[Attribute], visit: impl FnOnce(&mut Self)) {
        let own = cfgs_of(attrs);
        let len = self.cfg.len();
        self.cfg.extend(own);
        visit(self);
        self.cfg.truncate(len);
    }
}

impl<'ast> Visit<'ast> for ItemVisitor<'_> {
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.add_attr_calls(&item.ident, &item.attrs);
        let payload = item
            .variants
            .iter()
            .map(|variant| {
                let fields = variant
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        let name = field
                            .ident
                            .as_ref()
                            .map_or_else(|| index.to_string(), ToString::to_string);
                        (name, idents_of(&field.ty))
                    })
                    .collect();
                (variant.ident.to_string(), fields)
            })
            .collect();
        self.facts.enums.push(EnumFact {
            payload,
            krate: self.krate.clone(),
            name: item.ident.to_string(),
            variants: item.variants.iter().map(|v| v.ident.to_string()).collect(),
        });
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let public = matches!(item.vis, syn::Visibility::Public(_));
        self.add_fn(&item.attrs, &item.sig, &item.block, public, false);
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if is_test_item(&item.attrs) {
            return;
        }
        let mut generics = BTreeMap::new();
        generics_map(&item.generics, &mut generics);
        let scope = ImplScope {
            generics,
            owner: Some(last_ident(&item.self_ty).unwrap_or_else(|| "?".to_string())),
            trait_name: item
                .trait_
                .as_ref()
                .and_then(|(path, _)| path.segments.last().map(|s| s.ident.to_string())),
            self_args: self_type_args(&item.self_ty),
        };
        let outer = std::mem::replace(&mut self.scope, scope);
        self.with_cfg(&item.attrs, |visitor| {
            for member in &item.items {
                match member {
                    ImplItem::Fn(f) => {
                        let public = matches!(f.vis, syn::Visibility::Public(_));
                        visitor.add_fn(&f.attrs, &f.sig, &f.block, public, false);
                    }
                    ImplItem::Macro(m)
                        if m.mac
                            .path
                            .segments
                            .last()
                            .is_some_and(|s| s.ident == "delegate") =>
                    {
                        visitor.add_delegate(&m.mac);
                    }
                    _ => {}
                }
            }
        });
        self.scope = outer;
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if cfgs_of(&module.attrs).iter().any(|c| is_test_cfg(c)) || module.content.is_none() {
            return;
        }
        self.module.push(module.ident.to_string());
        self.with_cfg(&module.attrs, |visitor| {
            visit::visit_item_mod(visitor, module);
        });
        self.module.pop();
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.add_attr_calls(&item.ident, &item.attrs);
        let fields = item
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| index.to_string(), ToString::to_string);
                (name, idents_of(&field.ty))
            })
            .collect();
        let mut bounds = BTreeMap::new();
        generics_map(&item.generics, &mut bounds);
        self.facts.structs.push(StructFact {
            fields,
            bounds,
            krate: self.krate.clone(),
            name: item.ident.to_string(),
            params: item
                .generics
                .type_params()
                .map(|p| p.ident.to_string())
                .collect(),
        });
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        if is_test_item(&item.attrs) {
            return;
        }
        let name = item.ident.to_string();
        self.facts.traits.push(TraitFact {
            krate: self.krate.clone(),
            name: name.clone(),
        });
        let mut generics = BTreeMap::new();
        generics_map(&item.generics, &mut generics);
        generics.insert("Self".to_string(), vec![name.clone()]);
        let scope = ImplScope {
            generics,
            owner: Some(name.clone()),
            trait_name: Some(name),
            self_args: Vec::new(),
        };
        let outer = std::mem::replace(&mut self.scope, scope);
        self.with_cfg(&item.attrs, |visitor| {
            for member in &item.items {
                if let TraitItem::Fn(f) = member
                    && let Some(block) = &f.default
                {
                    visitor.add_fn(&f.attrs, &f.sig, block, true, true);
                }
            }
        });
        self.scope = outer;
    }

    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        self.facts.aliases.push(AliasFact {
            krate: self.krate.clone(),
            name: item.ident.to_string(),
            ty: idents_of(&item.ty),
        });
    }

    fn visit_item_use(&mut self, import: &'ast syn::ItemUse) {
        let mut out = Vec::new();
        use_tree(&import.tree, &mut Vec::new(), &mut out);
        for import in out {
            self.facts.uses.push(UseFact {
                import,
                krate: self.krate.clone(),
                module: self.module.clone(),
            });
        }
    }
}
