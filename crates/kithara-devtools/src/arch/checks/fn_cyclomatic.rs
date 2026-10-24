use anyhow::Result;
use syn::{
    BinOp, Block, Expr, ImplItem, Item, ItemImpl,
    visit::{self, Visit},
};

use super::{Check, Context};
use crate::common::{
    exclude::{attrs_are_test_only, attrs_have_test_marker, item_is_test_only},
    parse::{qualified, self_ty_name},
    violation::Violation,
    walker::{relative_to, workspace_rs_files_scoped},
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "fn_cyclomatic";
}

pub(crate) struct FnCyclomatic;

impl Check for FnCyclomatic {
    fn id(&self) -> &'static str {
        consts::ID
    }

    /// One violation per point of complexity above the limit, all under the
    /// function's key. The ratchet counts violations per key, so a function
    /// that is new above the limit fails as new and one that grows fails as a
    /// regression, while one that shrinks shows up as an improvement.
    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let limit = ctx.config.thresholds.fn_cyclomatic.deny;
        let mut violations = Vec::new();

        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let Some(file) = ctx.parsed_file(&path)? else {
                continue;
            };
            if attrs_are_test_only(&file.attrs) {
                continue;
            }
            let rel = relative_to(ctx.workspace_root, &path)
                .to_string_lossy()
                .replace('\\', "/");

            let mut hits: Vec<(String, usize)> = Vec::new();
            walk_items(&file.items, &mut Vec::new(), &mut hits);

            for (label, complexity) in hits {
                let key = format!("{rel}::{label}");
                let msg = format!(
                    "{label}: cyclomatic complexity {complexity} (limit {limit}); name the \
                     decisions as functions of their own"
                );
                for _ in limit..complexity {
                    violations.push(Violation::deny(consts::ID, key.clone(), msg.clone()));
                }
            }
        }
        Ok(violations)
    }
}

fn walk_items(items: &[Item], scope: &mut Vec<String>, out: &mut Vec<(String, usize)>) {
    for item in items {
        if item_is_test_only(item) {
            continue;
        }
        match item {
            Item::Fn(f) => out.push((
                qualified(scope, &f.sig.ident.to_string()),
                complexity(&f.block),
            )),
            Item::Impl(im) => walk_impl(im, scope, out),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    scope.push(m.ident.to_string());
                    walk_items(inner, scope, out);
                    scope.pop();
                }
            }
            _ => {}
        }
    }
}

fn walk_impl(im: &ItemImpl, scope: &[String], out: &mut Vec<(String, usize)>) {
    let owner = self_ty_name(&im.self_ty).unwrap_or_else(|| "?".to_string());
    for it in &im.items {
        if let ImplItem::Fn(f) = it
            && !attrs_are_test_only(&f.attrs)
            && !attrs_have_test_marker(&f.attrs)
        {
            out.push((
                qualified(scope, &format!("{owner}::{}", f.sig.ident)),
                complexity(&f.block),
            ));
        }
    }
}

/// Cyclomatic complexity: one path through the body plus one per `if`,
/// loop, `&&`/`||` and `?`, one per `match` and one more per guarded arm.
/// The compiler already proves a `match` exhaustive, so its unguarded arms
/// add no path to review, and a flat dispatch over an enum costs one. A
/// closure's decisions are its own, not the enclosing function's; items
/// nested in the body and macro input are not part of it either.
fn complexity(body: &Block) -> usize {
    let mut decisions = Decisions::default();
    decisions.visit_block(body);
    1 + decisions.count
}

#[derive(Default)]
struct Decisions {
    count: usize,
}

impl<'ast> Visit<'ast> for Decisions {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        self.count += match expr {
            Expr::Closure(_) => return,
            Expr::If(_) | Expr::While(_) | Expr::ForLoop(_) | Expr::Loop(_) | Expr::Try(_) => 1,
            Expr::Match(m) => {
                1 + m
                    .arms
                    .iter()
                    .filter(|arm| matches!(arm.pat, syn::Pat::Guard(_)))
                    .count()
            }
            Expr::Binary(b) if matches!(b.op, BinOp::And(_) | BinOp::Or(_)) => 1,
            _ => 0,
        };
        visit::visit_expr(self, expr);
    }

    fn visit_item(&mut self, _: &'ast Item) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(source: &str) -> Vec<(String, usize)> {
        let file = syn::parse_file(source).expect("fixture parses");
        let mut out = Vec::new();
        walk_items(&file.items, &mut Vec::new(), &mut out);
        out
    }

    #[test]
    fn every_decision_adds_one_path() {
        let source = r"
fn straight() { let a = 1; }
fn decisions(x: Option<u8>, y: bool) -> Option<u8> {
    if y && x.is_some() { return None; }
    for _ in 0..3 { while y || false {} }
    loop { break; }
    let r = match x { Some(0) => 1, Some(_) => 2, None => 3 };
    let f = |z: u8| if z > 1 { 1 } else { 0 };
    fn nested(a: bool) { if a {} if a {} }
    Some(r + f(r))?;
    None
}
";
        // if, &&, for, while, ||, loop, the match and `?`: eight decisions;
        // the closure's `if` and `nested` belong to themselves.
        assert_eq!(
            measured(source),
            [("straight".to_string(), 1), ("decisions".to_string(), 9)]
        );
    }

    #[test]
    fn a_flat_match_counts_once() {
        let arms: String = (0..20).map(|n| format!("{n} => {n},")).collect();
        let source = format!("fn flat(x: u8) -> u8 {{ match x {{ {arms} _ => 0 }} }}");
        assert_eq!(measured(&source), [("flat".to_string(), 2)]);
    }

    #[test]
    fn each_guarded_arm_adds_one() {
        let source = r"
fn guarded(x: u8, y: bool) -> u8 {
    match x { 0 if y => 1, 1 if !y => 2, 2 => 3, _ => 4 }
}
";
        assert_eq!(measured(source), [("guarded".to_string(), 4)]);
    }

    #[test]
    fn decisions_inside_arms_still_count() {
        let source = r"
fn arms(x: u8, v: Option<u8>) -> Option<u8> {
    Some(match x { 0 => v?, 1 => v? + 1, _ => v? + 2 })
}
";
        assert_eq!(measured(source), [("arms".to_string(), 5)]);
    }

    #[test]
    fn methods_are_named_by_their_owner_and_tests_are_skipped() {
        let source = r"
mod inner { impl Real { fn act(&self) { if true {} } } }
#[cfg(test)]
fn helper() { if true {} }
impl Real {
    #[test]
    fn check() { if true {} }
}
";
        assert_eq!(measured(source), [("inner::Real::act".to_string(), 2)]);
    }
}
