use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use syn::{
    Expr, ExprCall, ExprField, ExprMethodCall, ImplItemFn, Item, Member,
    visit::{self, Visit},
};

use crate::Ctx;

mod source;

use source::{Source, join, methods};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Report {
    pub(crate) notes: Vec<String>,
    pub(crate) types: Vec<TypeCohesion>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct TypeCohesion {
    pub(crate) location: String,
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) groups: Vec<Group>,
    pub(crate) lcom4: usize,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Group {
    pub(crate) fields: BTreeSet<String>,
    pub(crate) methods: BTreeSet<String>,
}

#[derive(Default)]
struct Method {
    calls: BTreeSet<String>,
    fields: BTreeSet<String>,
    name: String,
}

pub(crate) fn collect(
    ctx: &Ctx,
    tests: bool,
    selected: Option<&str>,
    selected_module: Option<&str>,
) -> Result<Report> {
    let excluded = ctx
        .config
        .architecture
        .filters
        .exclude_crates
        .iter()
        .map(|pattern| Pattern::new(pattern))
        .collect::<Result<Vec<_>, _>>()?;
    let mut report = Report { types: Vec::new(), notes: vec![
        "Source-level LCOM4 for structs: number of connected receiver-method groups; shared direct self fields and unambiguous self.method() and Self::method(self) calls connect methods. Associated functions and Drop are excluded; fieldless receiver methods remain isolated unless connected by calls.".to_owned(),
        "Diagnostic only: no debt, verdict, or CI threshold effect. Macros, deref/alias-based access, UFCS trait calls, generic dispatch, and glob imports are not resolved. Non-test cfg alternatives are combined; this is not a measurement of one compiled configuration.".to_owned(),
    ] };
    for package in ctx.metadata()?.workspace_packages() {
        if selected.is_some_and(|name| name != package.name.as_str())
            || selected.is_none()
                && !tests
                && excluded
                    .iter()
                    .any(|pattern| pattern.matches(package.name.as_str()))
        {
            continue;
        }
        for target in &package.targets {
            if !tests
                && (target.is_test()
                    || target.is_bench()
                    || target.is_example()
                    || target.is_custom_build())
            {
                continue;
            }
            let source = Source::read(&ctx.root, target.src_path.as_std_path(), tests)?;
            append_source(
                &mut report,
                &source,
                package.name.as_str(),
                &format!("{}:{:?}", target.name, target.kind),
                selected_module,
                tests,
            );
        }
    }
    report.types.sort_by(|left, right| {
        right
            .groups
            .len()
            .cmp(&left.groups.len())
            .then_with(|| (&left.name, &left.target).cmp(&(&right.name, &right.target)))
    });
    report.notes[2..].sort();
    report.notes.dedup();
    Ok(report)
}

fn append_source(
    report: &mut Report,
    source: &Source,
    package: &str,
    target: &str,
    selected: Option<&str>,
    tests: bool,
) {
    let mut types: BTreeMap<String, Vec<Method>> = BTreeMap::new();
    report.notes.extend(
        source
            .notes
            .iter()
            .map(|note| format!("{package}/{target}: {note}")),
    );
    for implementation in &source.implementations {
        let item = &implementation.item;
        let trait_name = item.trait_.as_ref().map(|(path, _)| {
            path.segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        });
        if trait_name
            .as_deref()
            .is_some_and(|name| matches!(name, "Drop" | "std::ops::Drop" | "core::ops::Drop"))
        {
            continue;
        }
        let Some(owner) = source.owner(implementation) else {
            if methods(item, tests).next().is_some() {
                report.notes.push(format!(
                    "{package}/{target} {}: impl owner unresolved; methods omitted",
                    implementation.location
                ));
            }
            continue;
        };
        if source
            .definitions
            .get(&owner)
            .is_some_and(|definition| definition.is_enum)
        {
            continue;
        }
        for method in methods(item, tests) {
            let name = trait_name.as_ref().map_or_else(
                || method.sig.ident.to_string(),
                |name| format!("<{name}>::{}", method.sig.ident),
            );
            types
                .entry(owner.clone())
                .or_default()
                .push(scan_method(method, name));
        }
    }
    for (name, definition) in &source.definitions {
        if selected.is_some_and(|module| {
            definition.module != module && !definition.module.starts_with(&format!("{module}::"))
        }) {
            continue;
        }
        if definition.is_enum {
            report.notes.push(format!(
                "{}: LCOM4 not applicable to enum variant state; not measured",
                join(package, name)
            ));
            continue;
        }
        let methods = types.remove(name).unwrap_or_default();
        if methods.is_empty() {
            continue;
        }
        let groups = groups(&methods);
        report.types.push(TypeCohesion {
            name: join(package, name),
            target: target.to_owned(),
            location: definition.location.clone(),
            lcom4: groups.len(),
            groups,
        });
    }
}

fn scan_method(method: &ImplItemFn, name: String) -> Method {
    let mut result = Method {
        name,
        ..Method::default()
    };
    result.visit_block(&method.block);
    result
}

impl<'ast> Visit<'ast> for Method {
    fn visit_expr_call(&mut self, expression: &'ast ExprCall) {
        if let Expr::Path(path) = expression.func.as_ref()
            && path.qself.is_none()
            && path.path.segments.len() == 2
            && path.path.segments[0].ident == "Self"
            && expression.args.first().is_some_and(is_self)
        {
            self.calls.insert(path.path.segments[1].ident.to_string());
        }
        visit::visit_expr_call(self, expression);
    }

    fn visit_expr_field(&mut self, expression: &'ast ExprField) {
        if is_self(&expression.base) {
            let field = match &expression.member {
                Member::Named(name) => name.to_string(),
                Member::Unnamed(index) => index.index.to_string(),
            };
            self.fields.insert(field);
        }
        visit::visit_expr_field(self, expression);
    }

    fn visit_expr_method_call(&mut self, expression: &'ast ExprMethodCall) {
        if is_self(&expression.receiver) {
            self.calls.insert(expression.method.to_string());
        }
        visit::visit_expr_method_call(self, expression);
    }

    fn visit_item(&mut self, _: &'ast Item) {}
}

fn is_self(expression: &Expr) -> bool {
    match expression {
        Expr::Path(path) => path.path.is_ident("self"),
        Expr::Reference(reference) => is_self(&reference.expr),
        Expr::Paren(paren) => is_self(&paren.expr),
        Expr::Group(group) => is_self(&group.expr),
        _ => false,
    }
}

/// ponytail: pairwise method scan; index field users if large types dominate scan time.
fn groups(methods: &[Method]) -> Vec<Group> {
    let mut names: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for method in methods {
        names
            .entry(method.name.rsplit("::").next().unwrap_or(&method.name))
            .or_default()
            .insert(&method.name);
    }
    let resolves = |caller: &Method, callee: &Method| {
        caller.calls.iter().any(|call| {
            if names
                .get(call.as_str())
                .is_some_and(|matches| matches.contains(call.as_str()))
            {
                callee.name == *call
            } else {
                names.get(call.as_str()).is_some_and(|matches| {
                    matches.len() == 1 && matches.contains(callee.name.as_str())
                })
            }
        })
    };
    let mut remaining: BTreeSet<usize> = (0..methods.len()).collect();
    let mut groups = Vec::new();
    while let Some(start) = remaining.pop_first() {
        let mut pending = vec![start];
        let mut group = Group {
            methods: BTreeSet::new(),
            fields: BTreeSet::new(),
        };
        while let Some(index) = pending.pop() {
            let method = &methods[index];
            group.methods.insert(method.name.clone());
            group.fields.extend(method.fields.iter().cloned());
            let connected = remaining
                .iter()
                .copied()
                .filter(|other| {
                    let other = &methods[*other];
                    method.name == other.name
                        || !method.fields.is_disjoint(&other.fields)
                        || resolves(method, other)
                        || resolves(other, method)
                })
                .collect::<Vec<_>>();
            for other in connected {
                remaining.remove(&other);
                pending.push(other);
            }
        }
        groups.push(group);
    }
    groups.sort_by(|left, right| left.methods.cmp(&right.methods));
    groups
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn lcom4_tracks_fields_calls_and_full_module_owners_across_files() {
        let temp = tempdir().expect("directory");
        fs::write(
            temp.path().join("lib.rs"),
            r#"
            mod first { pub struct Same { a: u8, b: u8 } }
            mod second { pub struct Same { a: u8 } impl Same { fn only(&self) { self.a; } } }
            mod behavior;
            #[cfg(test)] mod fixture;
        "#,
        )
        .expect("root");
        fs::write(
            temp.path().join("behavior.rs"),
            r#"
            use crate::first::Same as Owner;
            impl Owner {
                fn new() -> Self { todo!() }
                fn a(&self) { self.a; }
                fn b(&self) { self.b; }
                fn bridge(&self) { self.a(); Self::b(self); }
                fn alone(&self) {}
                #[cfg(test)] fn test_bridge(&self) { self.alone(); self.bridge(); }
            }
            impl Drop for Owner { fn drop(&mut self) { self.alone(); self.bridge(); } }
        "#,
        )
        .expect("impls");
        let source = Source::read(temp.path(), &temp.path().join("lib.rs"), false).expect("source");
        let mut report = Report {
            types: Vec::new(),
            notes: Vec::new(),
        };
        append_source(&mut report, &source, "demo", "lib", None, false);
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        assert_eq!(report.types.len(), 2);
        let first = &report.types[0];
        assert_eq!(first.name, "demo::first::Same");
        assert_eq!(first.groups.len(), 2);
        assert!(first.groups.iter().any(|group| group.methods
            == BTreeSet::from(["a".to_owned(), "b".to_owned(), "bridge".to_owned()])
            && group.fields == BTreeSet::from(["a".to_owned(), "b".to_owned()])));
        assert_eq!(report.types[1].groups.len(), 1);
    }
    #[test]
    fn lcom4_handles_tuple_fields_trait_names_and_unresolved_owners() {
        let temp = tempdir().expect("directory");
        fs::write(temp.path().join("lib.rs"), r#"
            struct Pair(u8, u8);
            enum State { Ready, Done }
            impl State { fn ready(&self) -> bool { matches!(self, Self::Ready) } }
            impl Pair { fn a(&self) { self.0; } fn same(&self) { self.0; } fn other(&self) { self.1; } }
            trait One { fn shared(&self); }
            trait Two { fn shared(&self); }
            impl One for Pair { fn shared(&self) { self.0; } }
            impl Two for Pair { fn shared(&self) { self.1; } }
            impl Pair { fn ambiguous(&self) { self.shared(); } }
            mod unrelated { impl Pair { fn bogus(&self) { self.0; self.1; } } }
        "#).expect("root");
        let source = Source::read(temp.path(), &temp.path().join("lib.rs"), false).expect("source");
        let mut report = Report {
            types: Vec::new(),
            notes: Vec::new(),
        };
        append_source(&mut report, &source, "demo", "lib", None, false);
        assert_eq!(report.types.len(), 1);
        assert_eq!(report.types[0].lcom4, 3);
        assert_eq!(report.notes.len(), 2);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("impl owner unresolved"))
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("LCOM4 not applicable to enum"))
        );
        assert!(
            report.types[0]
                .groups
                .iter()
                .any(|group| group.methods.contains("<One>::shared")
                    && group.methods.contains("same")
                    && group.fields.contains("0"))
        );
    }

    #[test]
    fn lcom4_module_scope_preserves_external_impls_and_complete_includes_tests() {
        let temp = tempdir().expect("directory");
        fs::write(
            temp.path().join("lib.rs"),
            r#"
            #[path = "model.rs"] mod data;
            mod operations;
            #[cfg_attr(feature = "x", path = "on.rs")]
            #[cfg_attr(not(feature = "x"), path = "off.rs")]
            mod conditional;
            #[cfg(test)] mod fixture { struct Fixture; impl Fixture { fn run(&self) {} } }
        "#,
        )
        .expect("root");
        fs::write(
            temp.path().join("model.rs"),
            "pub struct Data { a: u8, b: u8 }",
        )
        .expect("type");
        fs::write(
            temp.path().join("operations.rs"),
            r#"
            impl crate::data::Data {
                fn a(&self) { self.a; }
                fn b(&self) { self.b; }
                #[cfg(test)] fn bridge(&self) { self.a(); self.b(); }
            }
        "#,
        )
        .expect("impl");
        fs::write(temp.path().join("on.rs"), "struct On;").expect("conditional on");
        fs::write(temp.path().join("off.rs"), "struct Off;").expect("conditional off");
        for (tests, expected) in [(false, 2), (true, 1)] {
            let source =
                Source::read(temp.path(), &temp.path().join("lib.rs"), tests).expect("source");
            assert!(source.definitions.contains_key("conditional::On"));
            assert!(source.definitions.contains_key("conditional::Off"));
            let mut report = Report {
                types: Vec::new(),
                notes: Vec::new(),
            };
            append_source(&mut report, &source, "demo", "lib", Some("data"), tests);
            assert!(report.notes.is_empty());
            assert_eq!(report.types.len(), 1);
            assert_eq!(report.types[0].lcom4, expected);
        }
    }
}
