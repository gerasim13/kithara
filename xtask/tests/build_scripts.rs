//! Cargo reruns a build script when a path or variable it named changes, and
//! a script that names none is rerun whenever anything in its package is
//! newer than its last run. On a build directory checkouts share, that is
//! every claim. So every workspace build script names what it reads before it
//! can return, and none of them reads a value that changes on every commit.

use std::{fs, path::Path};

use syn::{Item, Stmt, visit::Visit};

/// Values that differ on every commit or pipeline: a script that reads one
/// reruns, and rebuilds everything that depends on it, on every job.
const PER_COMMIT: &[&str] = &[
    "CI_COMMIT_SHA",
    "GITHUB_SHA",
    "CI_PIPELINE_ID",
    "GITHUB_RUN_ID",
    "--git-path",
];

#[test]
fn every_build_script_tells_cargo_what_it_reads_before_it_can_return() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .expect("cargo metadata describes the workspace");
    let mut problems = Vec::new();
    let mut checked = 0;
    for package in &metadata.packages {
        for target in package
            .targets
            .iter()
            .filter(|target| target.is_custom_build())
        {
            let path = target.src_path.as_std_path();
            let source = fs::read_to_string(path).expect("a build script is readable");
            for value in PER_COMMIT {
                if source.contains(value) {
                    problems.push(format!(
                        "{}: reads {value}, which changes on every commit",
                        path.display()
                    ));
                }
            }
            let file = syn::parse_file(&source).expect("a build script parses");
            for item in &file.items {
                if let Item::Fn(main) = item
                    && main.sig.ident == "main"
                    && let Some(problem) = main_problem(&main.block.stmts)
                {
                    problems.push(format!("{}: {problem}", path.display()));
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "the workspace has build scripts");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// What is wrong with a `main` that can return, or ends, before it names a
/// path or a variable to cargo.
fn main_problem(stmts: &[Stmt]) -> Option<&'static str> {
    for stmt in stmts {
        if is_directive(stmt) {
            return None;
        }
        let mut exit = Exit::default();
        exit.visit_stmt(stmt);
        if exit.found {
            return Some("returns before printing a rerun-if directive");
        }
    }
    Some("prints no rerun-if directive")
}

/// A `println!` whose format string starts with a rerun-if directive.
fn is_directive(stmt: &Stmt) -> bool {
    let Stmt::Macro(stmt) = stmt else {
        return false;
    };
    stmt.mac.path.is_ident("println")
        && stmt
            .mac
            .tokens
            .clone()
            .into_iter()
            .next()
            .is_some_and(|token| {
                let text = token.to_string();
                text.starts_with("\"cargo::rerun-if") || text.starts_with("\"cargo:rerun-if")
            })
}

/// Whether a statement can leave `main`: a `return` or a `?` outside any
/// closure or nested item.
#[derive(Default)]
struct Exit {
    found: bool,
}

impl<'ast> Visit<'ast> for Exit {
    fn visit_expr_return(&mut self, _: &'ast syn::ExprReturn) {
        self.found = true;
    }

    fn visit_expr_try(&mut self, _: &'ast syn::ExprTry) {
        self.found = true;
    }

    /// A closure's `return` leaves the closure, not `main`.
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}

    /// A nested item's `return` leaves that item.
    fn visit_item(&mut self, _: &'ast Item) {}
}
