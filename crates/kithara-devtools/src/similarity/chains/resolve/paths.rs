use super::{Resolver, TypeDef, consts, dedup, module_key};
use crate::similarity::chains::{
    facts::path_segs,
    standard,
    ty::{ItemPath, Scope},
};

impl Resolver<'_> {
    pub(super) fn path(&self, path: &syn::Path, scope: Scope<'_>, depth: usize) -> Vec<ItemPath> {
        let segs = path_segs(path);
        if path.leading_colon.is_some() {
            self.external(&segs, depth, true)
        } else {
            self.paths(&segs, scope, depth)
        }
    }

    fn external(&self, path: &[String], depth: usize, allow_foreign: bool) -> Vec<ItemPath> {
        let Some((head, tail)) = path.split_first() else {
            return Vec::new();
        };
        if matches!(head.as_str(), "std" | "core" | "alloc") {
            return vec![standard::canonical(path)];
        }
        let krate = head.replace('_', "-");
        if self.crates.contains(krate.as_str()) {
            self.absolute(&krate, tail, depth, allow_foreign)
        } else if allow_foreign && !tail.is_empty() {
            vec![standard::canonical(&module_key(&krate, tail))]
        } else {
            Vec::new()
        }
    }

    pub(super) fn paths(&self, segs: &[String], scope: Scope<'_>, depth: usize) -> Vec<ItemPath> {
        if depth > consts::PATH_DEPTH {
            return Vec::new();
        }
        let Some((head, tail)) = segs.split_first() else {
            return Vec::new();
        };
        let place = scope.place;
        match head.as_str() {
            "crate" => return self.absolute(&place.krate, tail, depth, true),
            "self" => {
                return self.absolute(
                    &place.krate,
                    &[place.module.as_slice(), tail].concat(),
                    depth,
                    true,
                );
            }
            "super" => {
                let count = segs.iter().take_while(|part| *part == "super").count();
                let Some(parent) = place.module.get(..place.module.len().saturating_sub(count))
                else {
                    return Vec::new();
                };
                return self.absolute(
                    &place.krate,
                    &[parent, segs.get(count..).unwrap_or_default()].concat(),
                    depth,
                    true,
                );
            }
            _ => {}
        }
        let local: Vec<_> = scope
            .imports
            .iter()
            .filter(|import| import.alias == *head)
            .collect();
        if !local.is_empty() {
            return dedup(
                local
                    .iter()
                    .flat_map(|import| {
                        self.import_path(
                            &place.krate,
                            &place.module,
                            &[import.path.as_slice(), tail].concat(),
                            import.absolute,
                            depth + 1,
                            true,
                        )
                    })
                    .collect(),
            );
        }
        let own = self.absolute(
            &place.krate,
            &[place.module.as_slice(), segs].concat(),
            depth,
            true,
        );
        if !own.is_empty() {
            return own;
        }
        let scoped = self
            .uses
            .get(&module_key(&place.krate, &place.module))
            .into_iter()
            .flatten()
            .copied();
        let mut imports = scope.imports.iter().chain(scoped);
        if imports.clone().any(|import| import.alias == *head) {
            return Vec::new();
        }
        if imports.any(|import| {
            import.alias == "*"
                && self
                    .import_path(
                        &place.krate,
                        &place.module,
                        &import.path,
                        import.absolute,
                        depth + 1,
                        false,
                    )
                    .is_empty()
        }) {
            return Vec::new();
        }
        let external = self.external(segs, depth, true);
        if !external.is_empty() {
            return external;
        }
        if let Some(mut key) = standard::prelude(head) {
            key.extend(tail.iter().cloned());
            return vec![key];
        }
        Vec::new()
    }

    fn absolute(
        &self,
        krate: &str,
        path: &[String],
        depth: usize,
        allow_foreign: bool,
    ) -> Vec<ItemPath> {
        if depth > consts::PATH_DEPTH {
            return Vec::new();
        }
        if matches!(krate, "std" | "core" | "alloc") {
            return vec![standard::canonical(&module_key(krate, path))];
        }
        let key = module_key(krate, path);
        if self.types.contains_key(&key) || self.free.contains_key(&key) {
            return vec![key];
        }
        if path.split_last().is_some_and(|(name, prefix)| {
            self.types
                .get(&module_key(krate, prefix))
                .into_iter()
                .flatten()
                .any(|def| matches!(def, TypeDef::Enum(data) if data.variants.contains(name)))
        }) {
            return vec![key];
        }
        let cache_key = (key.clone(), depth, allow_foreign);
        if let Some(found) = self.path_memo.borrow().get(&cache_key) {
            return found.clone();
        }
        let mut out: Vec<_> = self
            .modules
            .contains(&key)
            .then_some(key)
            .into_iter()
            .collect();
        for (index, head) in path.iter().enumerate() {
            let module = path.get(..index).unwrap_or_default();
            let tail = path.get(index + 1..).unwrap_or_default();
            let Some(imports) = self.uses.get(&module_key(krate, module)) else {
                continue;
            };
            for import in imports {
                let relative = match import.alias.as_str() {
                    name if name == head => [import.path.as_slice(), tail].concat(),
                    "*" => [
                        import.path.as_slice(),
                        path.get(index..).unwrap_or_default(),
                    ]
                    .concat(),
                    _ => continue,
                };
                out.extend(self.import_path(
                    krate,
                    module,
                    &relative,
                    import.absolute,
                    depth + 1,
                    allow_foreign && import.alias != "*",
                ));
            }
        }
        let out = dedup(out);
        self.path_memo.borrow_mut().insert(cache_key, out.clone());
        out
    }

    fn import_path(
        &self,
        krate: &str,
        module: &[String],
        path: &[String],
        absolute: bool,
        depth: usize,
        allow_foreign: bool,
    ) -> Vec<ItemPath> {
        if depth > consts::PATH_DEPTH {
            return Vec::new();
        }
        if absolute {
            return self.external(path, depth, allow_foreign);
        }
        let Some((head, rest)) = path.split_first() else {
            return Vec::new();
        };
        match head.as_str() {
            "crate" => self.absolute(krate, rest, depth, allow_foreign),
            "self" => self.absolute(krate, &[module, rest].concat(), depth, allow_foreign),
            "super" => {
                let count = path.iter().take_while(|part| *part == "super").count();
                let Some(parent) = module.get(..module.len().saturating_sub(count)) else {
                    return Vec::new();
                };
                self.absolute(
                    krate,
                    &[parent, path.get(count..).unwrap_or_default()].concat(),
                    depth,
                    allow_foreign,
                )
            }
            _ => {
                let own = self.absolute(krate, &[module, path].concat(), depth, allow_foreign);
                if !own.is_empty() {
                    return own;
                }
                if self
                    .uses
                    .get(&module_key(krate, module))
                    .into_iter()
                    .flatten()
                    .any(|import| import.alias == *head)
                {
                    return Vec::new();
                }
                self.external(path, depth, allow_foreign)
            }
        }
    }
}
