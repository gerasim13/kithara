use anyhow::Result;
use syn::{FnArg, ImplItem, Item, ItemImpl, Signature};

use super::{Check, Context};
use crate::common::{
    exclude::{attrs_are_test_only, attrs_have_test_marker, item_is_test_only},
    parse::{qualified, self_ty_name},
    violation::Violation,
    walker::{relative_to, workspace_rs_files_scoped},
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "fn_arg_count";
}

pub(crate) struct FnArgCount;

impl Check for FnArgCount {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.fn_arg_count;
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

            for (label, count) in hits {
                if count < cfg.warn {
                    continue;
                }
                let key = format!("{rel}::{label}");
                let msg = format!(
                    "{label}: {count} parameters (warn threshold {}); consider a value \
                     object or splitting responsibilities",
                    cfg.warn
                );
                violations.push(Violation::warn(consts::ID, key, msg));
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
            Item::Fn(f) => {
                let n = count_args(&f.sig);
                if n > 0 {
                    out.push((qualified(scope, &f.sig.ident.to_string()), n));
                }
            }
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
            let n = count_args(&f.sig);
            if n > 0 {
                out.push((qualified(scope, &format!("{owner}::{}", f.sig.ident)), n));
            }
        }
    }
}

fn count_args(sig: &Signature) -> usize {
    sig.inputs
        .iter()
        .filter(|i| !matches!(i, FnArg::Receiver(_)))
        .count()
}

#[cfg(test)]
mod tests {
    use super::walk_items;

    #[test]
    fn argument_counts_follow_production_reachability() {
        let file = syn::parse_file(
            r#"
fn production(a: u8, b: u8) {}
#[cfg(all(
    test,
    feature = "fixtures",
))]
mod fixtures {
    fn helper(a: u8, b: u8, c: u8) {}
    mod nested { fn helper(a: u8) {} }
}
#[cfg(test)]
fn helper(a: u8) {}
#[cfg(all(test, feature = "fixtures"))]
impl Real { fn helper(&self, a: u8) {} }
impl Real {
    #[cfg(test)]
    fn helper(&self, a: u8) {}
    #[cfg(any(test, feature = "runtime"))]
    fn runtime(&self, a: u8, b: u8) {}
}
#[cfg(any(test, feature = "runtime"))]
fn runtime(a: u8) {}
#[cfg(not(test))]
fn production_branch(a: u8) {}
"#,
        )
        .expect("fixture parses");
        let mut counts = Vec::new();
        walk_items(&file.items, &mut Vec::new(), &mut counts);

        assert_eq!(
            counts,
            [
                ("production".to_string(), 2),
                ("Real::runtime".to_string(), 2),
                ("runtime".to_string(), 1),
                ("production_branch".to_string(), 1),
            ]
        );
    }
}
