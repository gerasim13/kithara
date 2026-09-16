use std::fs;

use anyhow::Result;
use syn::{Item, spanned::Spanned};

use super::{Check, Context};
use crate::{
    common::{
        violation::Violation,
        walker::{relative_to, workspace_rs_files_scoped},
    },
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableVariants;

impl Check for DerivableVariants {
    fn id(&self) -> &'static str {
        "derivable_variants"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_variants;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let source = fs::read_to_string(&path)?;
            let relative = relative_to(ctx.workspace_root, &path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message =
                    format!("enum/list macro {name}: use #[derive(kithara_derive::Variants)]");
                out.push(match config.severity {
                    DerivableSeverity::Deny => Violation::deny(self.id(), key, message),
                    DerivableSeverity::Warn => Violation::warn(self.id(), key, message),
                });
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}

fn check_source(source: &str) -> Vec<(String, usize)> {
    let Ok(file) = syn::parse_file(source) else {
        return Vec::new();
    };
    file.items
        .into_iter()
        .filter_map(|item| {
            let Item::Macro(item) = item else { return None };
            let name = item.ident?.to_string();
            let tokens = item.mac.tokens.to_string();
            (tokens.contains("enum") && tokens.contains("const ALL"))
                .then(|| (name, item.mac.path.span().start().line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_one_list_for_an_enum_and_all() {
        assert_eq!(check_source("macro_rules! values { ($($v:ident),*) => { enum Value { $($v),* } impl Value { const ALL: &'static [Self] = &[$(Self::$v),*]; } } }").len(), 1);
    }

    #[test]
    fn ignores_cross_schema_macros() {
        assert!(
            check_source(
                "macro_rules! values { ($($v:ident),*) => { struct Doc { $($v: String),* } } }"
            )
            .is_empty()
        );
    }
}
