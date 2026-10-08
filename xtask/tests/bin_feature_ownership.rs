use std::{fs, path::Path};

use cargo_metadata::MetadataCommand;
use syn::{Expr, ExprMethodCall, Lit, visit::Visit};

#[derive(Default)]
struct Requests(Vec<Vec<Option<String>>>);

impl<'ast> Visit<'ast> for Requests {
    fn visit_expr_method_call(&mut self, expression: &'ast ExprMethodCall) {
        if expression.method == "args"
            && let Some(Expr::Array(array)) = expression.args.first()
        {
            self.0.push(
                array
                    .elems
                    .iter()
                    .map(|element| match element {
                        Expr::Lit(literal) => match &literal.lit {
                            Lit::Str(value) => Some(value.value()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect(),
            );
        }
        syn::visit::visit_expr_method_call(self, expression);
    }
}

#[test]
fn explicit_binary_features_are_owned_by_the_manifest() {
    let tooling = Path::new(env!("CARGO_MANIFEST_DIR"));
    let metadata = MetadataCommand::new()
        .manifest_path(tooling.join("Cargo.toml"))
        .no_deps()
        .exec()
        .expect("workspace manifests resolve");
    let mut pending = vec![tooling.join("src")];
    let mut checked = 0;
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .expect("tooling source directory is readable")
                    .map(|entry| entry.expect("source directory entry").path()),
            );
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("source is readable");
        let parsed = syn::parse_file(&source).expect("source parses");
        let mut requests = Requests::default();
        requests.visit_file(&parsed);
        for arguments in requests.0 {
            let argument = |option: &str| {
                arguments
                    .iter()
                    .position(|value| value.as_deref() == Some(option))
                    .and_then(|index| arguments.get(index + 1))
                    .and_then(Option::as_deref)
            };
            let (Some(binary), Some(features)) = (argument("--bin"), argument("--features")) else {
                continue;
            };
            if !arguments
                .iter()
                .any(|value| value.as_deref() == Some("--no-default-features"))
            {
                continue;
            }
            let target = metadata
                .workspace_packages()
                .into_iter()
                .flat_map(|package| &package.targets)
                .find(|target| target.is_bin() && target.name == binary)
                .expect("requested binary is a workspace target");
            let mut required: Vec<_> = target
                .required_features
                .iter()
                .map(String::as_str)
                .collect();
            let mut requested: Vec<_> = features.split(',').collect();
            required.sort_unstable();
            requested.sort_unstable();
            assert_eq!(
                requested,
                required,
                "{}: binary {binary} must own its feature closure in required-features",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the source scan must exercise a binary request"
    );
}
