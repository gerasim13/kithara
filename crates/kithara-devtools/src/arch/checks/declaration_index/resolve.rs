use std::collections::BTreeSet;

use syn::{Type, TypePath};

use super::{
    graph::{DeclarationIndex, DeclarationKey, Kind},
    modules::known_attrs,
};

impl DeclarationIndex {
    pub(crate) fn key_for_decl(
        &self,
        rel: &str,
        inline: &[String],
        name: &str,
    ) -> Option<DeclarationKey> {
        let mut key = self.module(rel, inline)?;
        key.1.push(name.to_string());
        self.declarations.get(&key)?.as_ref()?;
        Some(key)
    }

    pub(crate) fn resolve_path(
        &self,
        rel: &str,
        inline: &[String],
        path: &syn::Path,
    ) -> Option<DeclarationKey> {
        let module = self.module(rel, inline)?;
        let names: Vec<_> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        self.resolve(
            &module,
            &names,
            path.leading_colon.is_some(),
            &mut BTreeSet::new(),
        )
    }

    pub(crate) fn resolve_function(
        &self,
        rel: &str,
        inline: &[String],
        path: &syn::Path,
    ) -> Option<DeclarationKey> {
        let key = self.resolve_path(rel, inline, path)?;
        (self.declarations.get(&key)? == &Some(Kind::Function)).then_some(key)
    }

    pub(crate) fn resolve_type(
        &self,
        rel: &str,
        inline: &[String],
        ty: &Type,
    ) -> Option<DeclarationKey> {
        let Type::Path(TypePath {
            qself: None, path, ..
        }) = ty
        else {
            return None;
        };
        let key = self.resolve_path(rel, inline, path)?;
        (self.declarations.get(&key)? == &Some(Kind::Type)).then_some(key)
    }

    pub(crate) fn resolve_impl(
        &self,
        rel: &str,
        inline: &[String],
        item: &syn::ItemImpl,
    ) -> Option<DeclarationKey> {
        if !known_attrs(&item.attrs) {
            return None;
        }
        let Type::Path(TypePath {
            qself: None, path, ..
        }) = item.self_ty.as_ref()
        else {
            return None;
        };
        if path.leading_colon.is_none()
            && let Some(first) = path.segments.first()
            && item
                .generics
                .type_params()
                .any(|parameter| parameter.ident == first.ident)
        {
            return None;
        }
        self.resolve_type(rel, inline, &item.self_ty)
    }

    fn module(&self, rel: &str, inline: &[String]) -> Option<DeclarationKey> {
        let locations = self.locations.get(&(rel.to_string(), inline.to_vec()))?;
        if locations.len() != 1 {
            return None;
        }
        let module = locations.first()?;
        for depth in 0..=module.1.len() {
            let ancestor = (module.0.clone(), module.1[..depth].to_vec());
            if self.modules.get(&ancestor)?.unknown {
                return None;
            }
        }
        Some(module.clone())
    }

    fn resolve(
        &self,
        module: &DeclarationKey,
        names: &[String],
        absolute: bool,
        seen: &mut BTreeSet<(DeclarationKey, String)>,
    ) -> Option<DeclarationKey> {
        let (first, rest) = names.split_first()?;
        if absolute {
            let root = self.externs.get(&module.0)?.get(first)?.as_ref()?;
            return self.descend(&(root.clone(), Vec::new()), rest, seen);
        }
        match first.as_str() {
            "crate" => self.descend(&(module.0.clone(), Vec::new()), rest, seen),
            "self" => self.descend(module, rest, seen),
            "super" => {
                let mut parent = module.clone();
                parent.1.pop()?;
                self.resolve(&parent, rest, false, seen)
            }
            _ => self.binding(module, first, rest, seen),
        }
    }

    fn descend(
        &self,
        module: &DeclarationKey,
        names: &[String],
        seen: &mut BTreeSet<(DeclarationKey, String)>,
    ) -> Option<DeclarationKey> {
        if self.modules.get(module)?.unknown {
            return None;
        }
        if names.is_empty() {
            return Some(module.clone());
        }
        self.binding(module, &names[0], &names[1..], seen)
    }

    fn binding(
        &self,
        module: &DeclarationKey,
        name: &str,
        rest: &[String],
        seen: &mut BTreeSet<(DeclarationKey, String)>,
    ) -> Option<DeclarationKey> {
        let scope = self.modules.get(module)?;
        if scope.unknown {
            return None;
        }
        let mut key = module.clone();
        key.1.push(name.to_string());
        let declaration = self.declarations.get(&key);
        let imports: Vec<_> = scope
            .imports
            .iter()
            .filter(|import| import.alias == name)
            .collect();
        if declaration.is_some() && !imports.is_empty() {
            return None;
        }
        if let Some(kind) = declaration {
            return match kind.as_ref()? {
                Kind::Module => self.descend(&key, rest, seen),
                _ if rest.is_empty() => Some(key),
                _ => None,
            };
        }
        if let [import] = imports.as_slice() {
            if !seen.insert((module.clone(), name.to_string())) {
                return None;
            }
            let mut path = import.path.clone();
            path.extend_from_slice(rest);
            return self.resolve(module, &path, import.absolute, seen);
        }
        if !imports.is_empty() || scope.imports.iter().any(|import| import.alias == "*") {
            return None;
        }
        let root = self.externs.get(&module.0)?.get(name)?.as_ref()?;
        self.descend(&(root.clone(), Vec::new()), rest, seen)
    }
}
