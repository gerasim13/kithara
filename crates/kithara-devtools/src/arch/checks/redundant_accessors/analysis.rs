use std::collections::BTreeMap;

use anyhow::Result;

use super::{
    super::{Check, Context, declaration_index::DeclarationKey},
    consts,
};
use crate::{
    arch::config::AccessorSeverity,
    common::{
        parse::{
            AccessKind, AccessPath, PassthroughOpts, collect_scopes, collect_self_field_writes,
            extract_passthrough_with, is_strict_pub, pub_methods, returns_handle_type,
        },
        violation::Violation,
        walker::{relative_to, workspace_rs_files_scoped},
    },
};

pub(crate) struct RedundantAccessors;

struct MethodFacts<'a> {
    fn_item: &'a syn::ImplItemFn,
    passthrough: Option<AccessPath>,
    name: String,
}

struct ImplSite<'a> {
    impl_block: &'a syn::ItemImpl,
    file_rel: String,
    mod_prefix: String,
}

type PubFieldsByType = BTreeMap<DeclarationKey, Vec<String>>;

/// One method in a target type's slice, paired with the file and module
/// path it was found in. Detectors use the file/module fields only to
/// build human-readable violation keys.
type MethodEntry<'a> = (MethodFacts<'a>, &'a str, &'a str);

impl Check for RedundantAccessors {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.redundant_accessors;
        let opts = PassthroughOpts {
            wrapper_ctors: cfg.wrapper_ctors.clone(),
            expose_methods: cfg.expose_methods.clone(),
        };

        let mut parsed: Vec<(String, &syn::File)> = Vec::new();
        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let Some(file) = ctx.parsed_file(&path)? else {
                continue;
            };
            let rel = relative_to(ctx.workspace_root, &path)
                .to_string_lossy()
                .replace('\\', "/");
            parsed.push((rel, file));
        }

        let declarations = ctx.declaration_index()?;
        let mut sites_by_type: BTreeMap<DeclarationKey, Vec<ImplSite<'_>>> = BTreeMap::new();
        let mut pub_fields_by_type: PubFieldsByType = BTreeMap::new();
        for (rel, file) in &parsed {
            for scope in collect_scopes(file) {
                let mod_prefix = if scope.path.is_empty() {
                    String::new()
                } else {
                    format!("{}::", scope.path.join("::"))
                };
                for s in &scope.structs {
                    let Some(identity) =
                        declarations.key_for_decl(rel, &scope.path, &s.ident.to_string())
                    else {
                        continue;
                    };
                    if let syn::Fields::Named(named) = &s.fields {
                        for f in &named.named {
                            if is_strict_pub(&f.vis)
                                && let Some(id) = &f.ident
                            {
                                pub_fields_by_type
                                    .entry(identity.clone())
                                    .or_default()
                                    .push(id.to_string());
                            }
                        }
                    }
                }
                for &im in &scope.impls {
                    if cfg.ignore_deref && is_deref_impl(im) {
                        continue;
                    }
                    let Some(identity) = declarations.resolve_impl(rel, &scope.path, im) else {
                        continue;
                    };
                    sites_by_type.entry(identity).or_default().push(ImplSite {
                        impl_block: im,
                        file_rel: rel.clone(),
                        mod_prefix: mod_prefix.clone(),
                    });
                }
            }
        }

        let mut violations = Vec::new();
        for (identity, sites) in &sites_by_type {
            let Some(target_type) = identity.1.last() else {
                continue;
            };
            let pub_fields = pub_fields_by_type
                .get(identity)
                .map_or(&[][..], Vec::as_slice);
            analyze_target_type(cfg, &opts, target_type, sites, pub_fields, &mut violations);
        }

        violations.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.message.cmp(&b.message)));
        violations.dedup_by(|a, b| a.key == b.key && a.message == b.message);
        Ok(violations)
    }
}

fn analyze_target_type(
    cfg: &crate::arch::config::RedundantAccessorsThreshold,
    opts: &PassthroughOpts,
    target_type: &str,
    sites: &[ImplSite<'_>],
    pub_fields: &[String],
    out: &mut Vec<Violation>,
) {
    let mut methods: Vec<MethodEntry<'_>> = Vec::new();
    let mut delegate_sites: Vec<(String, &str)> = Vec::new();
    for site in sites {
        for facts in collect_method_facts(site.impl_block, cfg.public_only, opts) {
            methods.push((facts, site.file_rel.as_str(), site.mod_prefix.as_str()));
        }
        for field in collect_delegate_targets(site.impl_block) {
            delegate_sites.push((field, site.file_rel.as_str()));
        }
    }

    if cfg.detect_field_passthrough {
        detect_p1(cfg, target_type, pub_fields, &methods, out);
    }
    if cfg.detect_nested_shorthand {
        detect_p2(cfg, target_type, &methods, out);
    }
    if cfg.detect_mutation_handle {
        detect_p3(cfg, target_type, &methods, out);
    }
    if cfg.detect_delegate_passthrough {
        detect_p4(cfg, target_type, &methods, &delegate_sites, out);
    }
}

/// Scan an `impl` block for `delegate! { to self.<field> { … } }` macro
/// invocations, returning the field names found at the top level. Recursion
/// into the brace-delimited body group is unnecessary — the `delegate` crate
/// places `to self.<field>` directly at the top of its macro body.
fn collect_delegate_targets(im: &syn::ItemImpl) -> Vec<String> {
    let mut out = Vec::new();
    for item in &im.items {
        let syn::ImplItem::Macro(macro_item) = item else {
            continue;
        };
        if !path_ends_with(&macro_item.mac.path, "delegate") {
            continue;
        }
        scan_to_self_field(macro_item.mac.tokens.clone(), &mut out);
    }
    out
}

fn path_ends_with(path: &syn::Path, name: &str) -> bool {
    path.segments.last().is_some_and(|seg| seg.ident == name)
}

fn scan_to_self_field(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
    use proc_macro2::TokenTree;

    let toks: Vec<TokenTree> = tokens.into_iter().collect();
    for (i, window) in toks.windows(4).enumerate() {
        let TokenTree::Ident(to_kw) = &window[0] else {
            continue;
        };
        if to_kw != "to" {
            continue;
        }
        let TokenTree::Ident(self_kw) = &window[1] else {
            continue;
        };
        if self_kw != "self" {
            continue;
        }
        let TokenTree::Punct(dot) = &window[2] else {
            continue;
        };
        if dot.as_char() != '.' {
            continue;
        }
        let TokenTree::Ident(field) = &window[3] else {
            continue;
        };
        if let Some(TokenTree::Punct(next)) = toks.get(i + 4)
            && next.as_char() == '.'
        {
            continue;
        }
        out.push(field.to_string());
    }
}

fn collect_method_facts<'a>(
    impl_block: &'a syn::ItemImpl,
    public_only: bool,
    opts: &PassthroughOpts,
) -> Vec<MethodFacts<'a>> {
    pub_methods(impl_block)
        .filter(|f| !public_only || is_strict_pub(&f.vis))
        .map(|f| MethodFacts {
            name: f.sig.ident.to_string(),
            fn_item: f,
            passthrough: extract_passthrough_with(f, opts),
        })
        .collect()
}

fn method_key(target_type: &str, m: &MethodEntry<'_>) -> String {
    let (facts, file_rel, mod_prefix) = m;
    format!("{file_rel}::{mod_prefix}{target_type}::{}", facts.name)
}

fn detect_p1(
    cfg: &crate::arch::config::RedundantAccessorsThreshold,
    target_type: &str,
    pub_fields: &[String],
    methods: &[MethodEntry<'_>],
    out: &mut Vec<Violation>,
) {
    for fname in pub_fields {
        for m in methods {
            let Some(p) = &m.0.passthrough else { continue };
            if p.fields.as_slice() == [fname.clone()] {
                let key = method_key(target_type, m);
                let msg = format!(
                    "P1: pub field `{fname}` is also exposed via `pub fn {}(&self)` ({:?}); \
                     pick one path",
                    m.0.name, p.kind
                );
                out.push(emit(cfg.p1_severity, key, msg));
            }
        }
    }
}

fn detect_p2(
    cfg: &crate::arch::config::RedundantAccessorsThreshold,
    target_type: &str,
    methods: &[MethodEntry<'_>],
    out: &mut Vec<Violation>,
) {
    let with_paths: Vec<(&MethodEntry<'_>, &AccessPath)> = methods
        .iter()
        .filter_map(|m| m.0.passthrough.as_ref().map(|p| (m, p)))
        .collect();

    for &(short_m, short_path) in &with_paths {
        if short_path.fields.len() < 2 {
            continue;
        }
        for &(container_m, container_path) in &with_paths {
            if container_m.0.name == short_m.0.name {
                continue;
            }
            if container_path.fields.len() >= short_path.fields.len()
                || !short_path.fields.starts_with(&container_path.fields)
            {
                continue;
            }
            let key = method_key(target_type, short_m);
            let chain = short_path.fields.join(".");
            let inner = container_path.fields.join(".");
            let tail = short_path.fields[container_path.fields.len()..].join(".");
            let msg = format!(
                "P2: `{}` is a shorthand for `{}().{tail}` \
                 (data path `self.{chain}` extends `self.{inner}`); pick one path",
                short_m.0.name, container_m.0.name,
            );
            out.push(emit(cfg.p2_severity, key, msg));
        }
    }
}

fn detect_p3(
    cfg: &crate::arch::config::RedundantAccessorsThreshold,
    target_type: &str,
    methods: &[MethodEntry<'_>],
    out: &mut Vec<Violation>,
) {
    for m in methods {
        let Some(p) = &m.0.passthrough else { continue };
        if p.fields.len() != 1
            || !matches!(
                p.kind,
                AccessKind::Ref | AccessKind::Clone | AccessKind::Move
            )
        {
            continue;
        }
        if !returns_handle_type(&m.0.fn_item.sig, &cfg.mutable_handle_types) {
            continue;
        }
        let exposed_field = &p.fields[0];

        for other in methods {
            if other.0.name == m.0.name {
                continue;
            }
            let writes = collect_self_field_writes(other.0.fn_item, &cfg.writer_methods);
            if writes.iter().any(|w| w == exposed_field) {
                let key = method_key(target_type, m);
                let msg = format!(
                    "P3: `{}` returns a handle to `self.{exposed_field}` (interior mutability) \
                     while `{}` already mutates the same field; external mutation through \
                     the handle bypasses the setter. \
                     Fix the *count* of write-paths, not their visibility: \
                     (a) delete this accessor and replace pull with a push event from the \
                     legitimate setter, or (b) move ownership of `{exposed_field}` to the \
                     consumer so the setter becomes a command. \
                     A read-only newtype wrapper around the same `Arc` does NOT fix this — \
                     it hides the second write-path behind a thin facade while the underlying \
                     `Arc<Atomic*/Mutex<...>>` is still cloneable and writable.",
                    m.0.name, other.0.name
                );
                out.push(emit(cfg.p3_severity, key, msg));
                break;
            }
        }
    }
}

fn detect_p4(
    cfg: &crate::arch::config::RedundantAccessorsThreshold,
    target_type: &str,
    methods: &[MethodEntry<'_>],
    delegate_sites: &[(String, &str)],
    out: &mut Vec<Violation>,
) {
    for (field, delegate_file) in delegate_sites {
        for m in methods {
            let Some(p) = &m.0.passthrough else { continue };
            if p.fields.as_slice() != [field.clone()] {
                continue;
            }
            if !matches!(
                p.kind,
                AccessKind::Ref | AccessKind::Clone | AccessKind::Move
            ) {
                continue;
            }
            let key = method_key(target_type, m);
            let cross_file_note = if m.1 == *delegate_file {
                String::new()
            } else {
                format!(" (delegate lives in `{delegate_file}`)")
            };
            let msg = format!(
                "P4: `{field}` is exposed both via `pub fn {}(&self)` and via \
                 `delegate! {{ to self.{field} {{ … }} }}`{cross_file_note}; pick one path \
                 (drop the accessor and keep `delegate!`, or drop the macro \
                 and route external callers through the accessor)",
                m.0.name
            );
            out.push(emit(cfg.p4_severity, key, msg));
        }
    }
}

fn emit(sev: AccessorSeverity, key: String, message: String) -> Violation {
    match sev {
        AccessorSeverity::Deny => Violation::deny(consts::ID, key, message),
        AccessorSeverity::Warn => Violation::warn(consts::ID, key, message),
        AccessorSeverity::Off => unreachable!("off severity should be filtered earlier"),
    }
}

fn is_deref_impl(im: &syn::ItemImpl) -> bool {
    let Some((path, _)) = &im.trait_ else {
        return false;
    };
    let Some(last) = path.segments.last() else {
        return false;
    };
    matches!(last.ident.to_string().as_str(), "Deref" | "DerefMut")
}
