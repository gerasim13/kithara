use std::{fs, path::Path};

use anyhow::Result;
use syn::{Expr, Lit, Meta, Token, punctuated::Punctuated, visit::Visit};
use url::Url;

use crate::common::project::ProjectConfig;

pub(crate) fn declarations(file: &syn::File) -> Vec<(String, usize, Vec<Option<String>>)> {
    let mut ignored = IgnoredFunctions::default();
    ignored.visit_file(file);
    ignored.functions
}

#[derive(Default)]
struct IgnoredFunctions {
    functions: Vec<(String, usize, Vec<Option<String>>)>,
}

impl<'ast> Visit<'ast> for IgnoredFunctions {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        let mut reasons = Vec::new();
        for attribute in &function.attrs {
            collect_ignore_reasons(&attribute.meta, &mut reasons);
        }
        if !reasons.is_empty() {
            self.functions.push((
                function.sig.ident.to_string(),
                function.sig.fn_token.span.start().line,
                reasons,
            ));
        }
        syn::visit::visit_item_fn(self, function);
    }
}

fn collect_ignore_reasons(meta: &Meta, reasons: &mut Vec<Option<String>>) {
    if meta.path().is_ident("ignore") {
        let reason = match meta {
            Meta::NameValue(value) => match &value.value {
                Expr::Lit(literal) => match &literal.lit {
                    Lit::Str(reason) => Some(reason.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        reasons.push(reason);
    } else if let Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
        && let Ok(nested) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
    {
        for attribute in nested.iter().skip(1) {
            collect_ignore_reasons(attribute, reasons);
        }
    }
}

pub(crate) fn reason_is_owned(reason: &str, root: &Path, project: &ProjectConfig) -> Result<bool> {
    let mut owned = false;
    let mut valid = true;
    let mut issue = false;
    let mut nightly = false;
    for field in reason.split(';').map(str::trim) {
        if let Some(lane) = field.strip_prefix("lane:") {
            let known = project.test.lanes.contains_key(lane.trim());
            valid &= known;
            owned |= known;
        } else if let Some(command) = field.strip_prefix("run:") {
            let known = command_is_declared(command.trim(), root, project)?;
            valid &= known;
            owned |= known;
        } else if let Some(url) = field.strip_prefix("issue:") {
            let known = issue_is_valid(url.trim());
            valid &= known;
            issue |= known;
            owned |= known;
        } else if let Some(kind) = field.strip_prefix("nightly:") {
            nightly = true;
            valid &= matches!(kind.trim(), "red" | "flake");
        }
    }
    Ok(owned && valid && (!nightly || issue))
}

fn issue_is_valid(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let segments = url.path().split('/').collect::<Vec<_>>();
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(segments.as_slice(), ["", owner, repository, "issues", number]
            if !owner.is_empty() && !repository.is_empty()
                && number.parse::<u64>().is_ok_and(|number| number > 0))
}

fn command_is_declared(command: &str, root: &Path, project: &ProjectConfig) -> Result<bool> {
    let mut words = command.split_whitespace();
    if words.next() != Some("just") {
        return Ok(false);
    }
    let Some(first) = words.next() else {
        return Ok(false);
    };
    let source = fs::read_to_string(root.join("justfile"))?;
    let module = source.lines().find_map(|line| {
        let mut declaration = line.split_whitespace();
        (declaration.next() == Some("mod") && declaration.next() == Some(first))
            .then(|| declaration.next())
            .flatten()
    });
    let (source, recipe) = match module {
        Some(path) => {
            let Some(recipe) = words.next() else {
                return Ok(false);
            };
            let path = path.trim_matches(['\'', '"']);
            (fs::read_to_string(root.join(path))?, recipe)
        }
        None => (source, first),
    };
    let declared = source.lines().any(|line| {
        !line.starts_with(char::is_whitespace)
            && line.split_once(':').is_some_and(|(signature, body)| {
                !body.starts_with('=') && signature.split_whitespace().next() == Some(recipe)
            })
    });
    let arguments = words.collect::<Vec<_>>();
    let lanes_valid = arguments.iter().enumerate().all(|(index, argument)| {
        if let Some(lane) = argument.strip_prefix("--lane=") {
            project.test.lanes.contains_key(lane)
        } else if *argument == "--lane" {
            arguments
                .get(index + 1)
                .is_some_and(|lane| project.test.lanes.contains_key(*lane))
        } else {
            true
        }
    });
    Ok(declared && lanes_valid)
}
