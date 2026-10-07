use std::{collections::BTreeMap, fs};

use anyhow::Result;
use syn::{Expr, ImplItem, ItemImpl, Stmt};

use super::{Check, Context};
use crate::{
    common::{
        parse::{collect_scopes, self_ty_name},
        violation::Violation,
        walker::relative_to,
    },
    idioms::config::DerivableSeverity,
};

pub(crate) struct DerivableSkinWalk;

type TraversalKey = (Vec<String>, String);
type TraversalLines = (Option<usize>, Option<usize>);

impl Check for DerivableSkinWalk {
    fn id(&self) -> &'static str {
        "derivable_skin_walk"
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let config = &ctx.config.thresholds.derivable_skin_walk;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let source = fs::read_to_string(path)?;
            let relative = relative_to(ctx.workspace_root, path).to_string_lossy();
            for (name, line) in check_source(&source) {
                let key = format!("{relative}:{line}:0");
                let message = format!(
                    "structural skin traversal for {name}: replace manual Frames/Roles impls with #[derive(SkinWalk)]"
                );
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
    let mut candidates = BTreeMap::<TraversalKey, TraversalLines>::new();
    for scope in collect_scopes(&file) {
        for implementation in scope.impls {
            let Some((trait_path, _)) = &implementation.trait_ else {
                continue;
            };
            let Some(trait_name) = trait_path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            else {
                continue;
            };
            if !matches!(trait_name.as_str(), "Frames" | "Roles") || !is_projection(implementation)
            {
                continue;
            }
            let Some(name) = self_ty_name(&implementation.self_ty) else {
                continue;
            };
            let line = implementation.impl_token.span.start().line;
            let entry = candidates.entry((scope.path.clone(), name)).or_default();
            if trait_name == "Frames" {
                entry.0 = Some(line);
            } else {
                entry.1 = Some(line);
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|((_, name), (frames, roles))| Some((name, frames?.min(roles?))))
        .collect()
}

fn is_projection(implementation: &ItemImpl) -> bool {
    let [ImplItem::Fn(method)] = implementation.items.as_slice() else {
        return false;
    };
    if !matches!(
        method.sig.ident.to_string().as_str(),
        "each_frame" | "each_role"
    ) || method.block.stmts.is_empty()
    {
        return false;
    }
    method.block.stmts.iter().all(|statement| {
        let Stmt::Expr(Expr::MethodCall(call), _) = statement else {
            return false;
        };
        matches!(call.receiver.as_ref(), Expr::Field(field) if matches!(field.base.as_ref(), Expr::Path(path) if path.path.is_ident("self")))
            && matches!(call.method.to_string().as_str(), "each_frame" | "each_role")
    })
}

#[cfg(test)]
mod tests {
    use super::check_source;

    #[test]
    fn finds_paired_structural_traversal() {
        let source = r#"
            impl Frames for Section {
                fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) {
                    self.frame.each_frame(visit);
                }
            }
            impl Roles for Section {
                fn each_role(&mut self, visit: &mut dyn FnMut(&mut TextRoleSkin)) {
                    self.label.each_role(visit);
                }
            }
        "#;
        assert_eq!(check_source(source)[0].0, "Section");
    }

    #[test]
    fn ignores_leaf_container_and_runtime_branch_shapes() {
        let source = r#"
            impl Frames for FrameSkin {
                fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) { visit(self); }
            }
            impl<T: Frames> Frames for Option<T> {
                fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) {
                    if let Some(value) = self { value.each_frame(visit); }
                }
            }
            impl Frames for WindowControlSkin {
                fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) {
                    match self { Self::Close { frame, .. } => frame.each_frame(visit), _ => {} }
                }
            }
        "#;
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn pairs_traversal_only_inside_the_same_test_module() {
        let source = r#"
            impl Frames for Section { fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) { self.frame.each_frame(visit); } }
            #[cfg(test)] mod tests {
                impl Frames for Section { fn each_frame(&mut self, visit: &mut dyn FnMut(&mut FrameSkin)) { self.frame.each_frame(visit); } }
                impl Roles for Section { fn each_role(&mut self, visit: &mut dyn FnMut(&mut TextRoleSkin)) { self.label.each_role(visit); } }
            }
        "#;
        assert_eq!(check_source(source), [("Section".to_owned(), 4)]);
    }
}
