use std::{collections::BTreeSet, fs, path::PathBuf};

use syn::{Item, Type, Visibility, spanned::Spanned};

use super::facts::{AliasFact, Facts, Place};

impl Facts {
    pub(super) fn add_dependency_aliases(&mut self, roots: &BTreeSet<(String, PathBuf)>) {
        for (krate, path) in roots {
            let text = match fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    tracing::debug!(%error, root = %path.display(), "skip dependency aliases");
                    continue;
                }
            };
            let file = match syn::parse_file(&text) {
                Ok(file) => file,
                Err(error) => {
                    tracing::debug!(%error, root = %path.display(), "skip dependency aliases");
                    continue;
                }
            };
            for item in file.items {
                if let Item::Type(item) = item
                    && matches!(item.vis, Visibility::Public(_))
                    && matches!(&*item.ty, Type::Path(path) if path.qself.is_none())
                {
                    self.aliases.push(AliasFact::new(
                        Place {
                            file: path.display().to_string(),
                            krate: krate.replace('_', "-"),
                            module: Vec::new(),
                            line: item.span().start().line,
                            end: item.span().end().line,
                        },
                        &item,
                    ));
                }
            }
        }
    }
}
