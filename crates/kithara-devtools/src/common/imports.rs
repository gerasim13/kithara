use syn::UseTree;

/// A `use` binding: the name it brings into scope and the path it names. A
/// glob import has the alias `*`.
#[derive(Clone, Debug)]
pub(crate) struct Import {
    pub(crate) alias: String,
    pub(crate) path: Vec<String>,
    pub(crate) absolute: bool,
}

/// Flattens a `use` tree into its bindings.
pub(crate) fn use_tree(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<Import>) {
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
            out.push(Import {
                alias,
                path,
                absolute: false,
            });
        }
        UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            if rename.ident != "self" {
                path.push(rename.ident.to_string());
            }
            out.push(Import {
                path,
                alias: rename.rename.to_string(),
                absolute: false,
            });
        }
        UseTree::Glob(_) => out.push(Import {
            alias: "*".to_string(),
            path: prefix.clone(),
            absolute: false,
        }),
        UseTree::Group(group) => {
            for item in &group.items {
                use_tree(item, prefix, out);
            }
        }
    }
}
