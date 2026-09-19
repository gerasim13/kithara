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

pub(crate) struct DerivableEnumStr;

impl Check for DerivableEnumStr {
    fn id(&self) -> &'static str {
        "derivable_enum_str"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_enum_str;
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
                    format!("enum vocabulary macro {name}: use #[derive(kithara_derive::EnumStr)]");
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
    let mut out = Vec::new();
    collect_macros(&file.items, &mut out);
    out
}

fn collect_macros(items: &[Item], out: &mut Vec<(String, usize)>) {
    for item in items {
        match item {
            Item::Macro(item) => {
                let Some(name) = item.ident.as_ref().map(ToString::to_string) else {
                    continue;
                };
                let tokens = item.mac.tokens.to_string();
                if tokens.contains("const")
                    && tokens.contains("fn")
                    && tokens.contains("match")
                    && tokens.contains("stringify")
                {
                    out.push((name, item.mac.path.span().start().line));
                }
            }
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_macros(nested, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn reports_parallel_names_and_match() {
        assert_eq!(check_source("macro_rules! names { ($($v:ident),*) => { impl Value { const ALL: &'static [&'static str] = &[$(stringify!($v)),*]; fn name(&self) -> &'static str { match self { $(Self::$v => stringify!($v)),* } } } } }").len(), 1);
    }

    #[test]
    fn ignores_a_semantic_match_macro() {
        assert!(
            check_source("macro_rules! map { ($v:expr) => { match $v { 0 => 1, _ => 2 } } }")
                .is_empty()
        );
    }

    #[test]
    fn reports_a_macro_in_a_test_module() {
        assert_eq!(check_source("#[cfg(test)] mod tests { macro_rules! names { ($($v:ident),*) => { impl Value { const ALL: &'static [&'static str] = &[$(stringify!($v)),*]; fn name(&self) -> &'static str { match self { $(Self::$v => stringify!($v)),* } } } } } }").len(), 1);
    }
}
