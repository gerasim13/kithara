use anyhow::Result;
use syn::{
    Attribute, Expr, Field, FnArg, ImplItemFn, ItemConst, ItemFn, ItemMod, ItemStatic, ItemStruct,
    Local, Member, Pat, PatIdent, ReceiverKind, ReturnType, Safety, Stmt, Type,
    visit::{self, Visit},
};

use super::{Check, Context};
use crate::{
    common::{
        suppress::Suppressions,
        violation::Violation,
        walker::{compile_globs, matches_any, relative_to},
    },
    idioms::config::RetryFallbackConfig,
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "retry_fallback";

    pub(super) const EXPLANATION: &str = "\
Detected a retry/attempt counter or try-then-fallback chain. Both \
patterns paper over a broken primary path: if the first call may fail, \
fix the contract — don't hide the bug behind N attempts or a chain of \
alternative implementations.

Why it matters. `attempt`, `retries`, `max_retries`, and `fallback` \
fields turn a single algorithmic failure into a per-call lottery. The \
test that fails 1-in-10 with a retry-3 wrapper hides a real race; the \
production path that 'falls back to B if A fails' double-encodes the \
problem (A is wrong, B is also wrong, the contract is wrong). They \
also accumulate: each new attempt grows the surface for new races, \
each fallback hides another underlying failure mode.

❌  if request.attempt == 0 { try_seek() } else if request.attempt < MAX { retry() }
✅  fix `try_seek` so it always lands or returns a typed error the caller \
   handles deterministically.

❌  fn read_or_fallback(...) -> Bytes { read_primary().unwrap_or_else(read_secondary) }
✅  extend the owner's algorithm so one path covers the case, or model \
   the choice as user-facing config; don't chain implementations.

Exact identifiers listed in `retry_fallback.allowed_idents` are excluded from \
this lexical check. Borrowed configuration setters that only clone settings, \
assign the supplied value, and construct a handle are not execution retries; \
their bodies remain checked.

Suppress with `// xtask-lint-ignore: retry_fallback` ONLY for a designed \
fallback the owner's contract names: a user-facing default, optional \
config, or degraded mode (e.g. a config field literally named \
`fallback_url` where the user opted in to two endpoints). Suppression \
for control flow is a code smell that should be discussed and fixed, not \
silenced.";
}

pub(crate) struct RetryFallback;

impl Check for RetryFallback {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.retry_fallback;
        let exempt = compile_globs(&cfg.exempt_files);
        let mut violations = Vec::new();
        for path in ctx.scan.rs_files(ctx.scope)?.iter() {
            let rel_path = relative_to(ctx.workspace_root, path).to_path_buf();
            let rel = rel_path.to_string_lossy().replace('\\', "/");
            if matches_any(&exempt, std::path::Path::new(&rel)) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(file) = ctx.scan.parse_file(path) else {
                continue;
            };
            let suppress = Suppressions::parse(&source);
            let mut v = IdentVisitor {
                cfg,
                rel: &rel,
                suppress: &suppress,
                out: &mut violations,
                inside_test_mod: false,
            };
            v.visit_file(&file);
        }
        Ok(violations)
    }
}

struct IdentVisitor<'a> {
    cfg: &'a RetryFallbackConfig,
    suppress: &'a Suppressions,
    out: &'a mut Vec<Violation>,
    rel: &'a str,
    /// `true` while traversing inside a `#[cfg(test)]` module — test code
    /// can legitimately use names like `flags_max_retries_const` that
    /// describe the rule's own behaviour without smelling like a retry.
    inside_test_mod: bool,
}

/// Returns `true` if any of the given attributes is `#[cfg(test)]`.
fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let mut is_test = false;
        let _ = a.parse_nested_meta(|m| {
            if m.path.is_ident("test") {
                is_test = true;
            }
            Ok(())
        });
        is_test
    })
}

impl<'a> IdentVisitor<'a> {
    fn flag(&mut self, line: usize, name: &str, kind: &str) {
        if self.inside_test_mod || self.suppress.is_suppressed(line, consts::ID) {
            return;
        }
        let key = format!("{}:{line}:{name}", self.rel);
        let message = format!(
            "{rel}:{line} — {kind} `{name}` encodes a retry/fallback. Fix \
             the primary contract instead of counting attempts or chaining \
             alternatives.",
            rel = self.rel,
            line = line,
            kind = kind,
            name = name,
        );
        self.out
            .push(Violation::deny(consts::ID, key, message).with_explanation(consts::EXPLANATION));
    }
}

fn name_is_forbidden(name: &str, cfg: &RetryFallbackConfig) -> bool {
    if cfg.allowed_idents.iter().any(|allowed| allowed == name) {
        return false;
    }
    let lower = name.to_lowercase();
    if cfg.forbidden_idents.contains(&lower) {
        return true;
    }
    cfg.forbidden_substrings
        .iter()
        .any(|fragment| lower.contains(fragment.as_str()))
}

fn is_configuration_setter(method: &ImplItemFn, cfg: &RetryFallbackConfig) -> bool {
    let signature = &method.sig;
    if !signature.ident.to_string().starts_with("with_")
        || signature.asyncness.is_some()
        || signature.constness.is_some()
        || !matches!(signature.safety, Safety::Default)
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || signature.inputs.len() != 2
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
    {
        return false;
    }
    let (Some(FnArg::Receiver(receiver)), Some(FnArg::Typed(value))) =
        (signature.inputs.first(), signature.inputs.last())
    else {
        return false;
    };
    if !matches!(receiver.kind, ReceiverKind::Reference(_, None, None))
        || receiver.mutability.is_some()
        || matches!(value.ty.as_ref(), Type::Reference(_))
    {
        return false;
    }
    let Pat::Ident(argument) = value.pat.as_ref() else {
        return false;
    };
    if argument.by_ref.is_some() || argument.mutability.is_some() || argument.subpat.is_some() {
        return false;
    }
    if !matches!(&signature.output, ReturnType::Type(_, ty)
        if matches!(ty.as_ref(), Type::Path(path)
            if path.qself.is_none() && path.path.is_ident("Self")))
    {
        return false;
    }
    let [
        Stmt::Local(local),
        Stmt::Expr(Expr::Assign(assign), Some(_)),
        Stmt::Expr(Expr::MethodCall(construct), None),
    ] = method.block.stmts.as_slice()
    else {
        return false;
    };
    let Pat::Ident(settings) = &local.pat else {
        return false;
    };
    if settings.mutability.is_none()
        || settings.by_ref.is_some()
        || settings.subpat.is_some()
        || !local.attrs.is_empty()
    {
        return false;
    }
    let Some(initializer) = &local.init else {
        return false;
    };
    let Expr::MethodCall(clone) = initializer.expr.as_ref() else {
        return false;
    };
    let Expr::MethodCall(getter) = clone.receiver.as_ref() else {
        return false;
    };
    let Expr::Field(field) = assign.left.as_ref() else {
        return false;
    };
    initializer.diverge.is_none()
        && clone.method == "clone"
        && clone.args.is_empty()
        && clone.turbofish.is_none()
        && getter.args.is_empty()
        && getter.turbofish.is_none()
        && expr_is_ident(&getter.receiver, "self")
        && !name_is_forbidden(&getter.method.to_string(), cfg)
        && matches!(field.member, Member::Named(_))
        && expr_is_ident(&field.base, &settings.ident.to_string())
        && expr_is_ident(&assign.right, &argument.ident.to_string())
        && expr_is_ident(&construct.receiver, "self")
        && construct.method.to_string().starts_with("with_")
        && !name_is_forbidden(&construct.method.to_string(), cfg)
        && construct.turbofish.is_none()
        && construct.args.len() == 1
        && construct
            .args
            .first()
            .is_some_and(|value| expr_is_ident(value, &settings.ident.to_string()))
}

fn expr_is_ident(expr: &Expr, ident: &str) -> bool {
    matches!(expr, Expr::Path(path) if path.qself.is_none() && path.path.is_ident(ident))
}

impl<'ast> Visit<'ast> for IdentVisitor<'_> {
    fn visit_field(&mut self, node: &'ast Field) {
        if let Some(ident) = &node.ident {
            let name = ident.to_string();
            if name_is_forbidden(&name, self.cfg) {
                self.flag(ident.span().start().line, &name, "field");
            }
        }
        visit::visit_field(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let name = node.sig.ident.to_string();
        if name_is_forbidden(&name, self.cfg) && !is_configuration_setter(node, self.cfg) {
            self.flag(node.sig.ident.span().start().line, &name, "fn");
        }
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast ItemConst) {
        let name = node.ident.to_string();
        if name_is_forbidden(&name, self.cfg) {
            self.flag(node.ident.span().start().line, &name, "const");
        }
        visit::visit_item_const(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        if name_is_forbidden(&name, self.cfg) {
            self.flag(node.sig.ident.span().start().line, &name, "fn");
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        let was_inside = self.inside_test_mod;
        if is_cfg_test(&node.attrs) {
            self.inside_test_mod = true;
        }
        visit::visit_item_mod(self, node);
        self.inside_test_mod = was_inside;
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        let name = node.ident.to_string();
        if name_is_forbidden(&name, self.cfg) {
            self.flag(node.ident.span().start().line, &name, "static");
        }
        visit::visit_item_static(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        let name = node.ident.to_string();
        if name_is_forbidden(&name, self.cfg) {
            self.flag(node.ident.span().start().line, &name, "struct");
        }
        visit::visit_item_struct(self, node);
    }

    fn visit_local(&mut self, node: &'ast Local) {
        if let Pat::Ident(PatIdent { ident, .. }) = &node.pat {
            let name = ident.to_string();
            if name_is_forbidden(&name, self.cfg) {
                self.flag(ident.span().start().line, &name, "let");
            }
        }
        visit::visit_local(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIGURATION_SETTER: &str = r"
        impl Client {
            pub fn with_retry_policy(&self, retry_policy: RetryPolicy) -> Self {
                let mut options = self.options().clone();
                options.retry_policy = retry_policy;
                self.with_options(options)
            }
        }
    ";

    fn count_violations(source: &str) -> usize {
        count_violations_with_allowed(source, &[])
    }

    fn count_violations_with_allowed(source: &str, allowed_idents: &[&str]) -> usize {
        let file = syn::parse_file(source).expect("parse");
        let suppress = Suppressions::parse(source);
        let cfg = RetryFallbackConfig {
            allowed_idents: allowed_idents.iter().map(|s| (*s).to_owned()).collect(),
            ..RetryFallbackConfig::default()
        };
        let mut out = Vec::new();
        let mut v = IdentVisitor {
            cfg: &cfg,
            rel: "test.rs",
            suppress: &suppress,
            out: &mut out,
            inside_test_mod: false,
        };
        v.visit_file(&file);
        out.len()
    }

    #[test]
    fn flags_attempt_field() {
        let src = "struct Req { attempt: u8 }\n";
        assert_eq!(count_violations(src), 1);
    }

    #[test]
    fn flags_max_retries_const() {
        let src = "const MAX_RETRIES: u8 = 3;\n";
        assert_eq!(count_violations(src), 1);
    }

    #[test]
    fn flags_fallback_fn() {
        let src = "fn read_or_fallback() {}\n";
        assert_eq!(count_violations(src), 1);
    }

    #[test]
    fn flags_let_attempts() {
        let src = "fn f() { let attempts = 0; }\n";
        assert_eq!(count_violations(src), 1);
    }

    #[test]
    fn allows_configured_identifiers() {
        let src = "struct RetryTelemetry { attempt: u32, max_retries: u32 }\n\
            fn publish() { let attempt = 1; }\n";
        assert_eq!(
            count_violations_with_allowed(src, &["attempt", "max_retries"]),
            0
        );
    }

    #[test]
    fn configured_identifiers_match_exactly() {
        let src = "struct RetryTelemetry { attempt: u32, attempts: u32 }\n";
        assert_eq!(count_violations_with_allowed(src, &["attempt"]), 1);
    }

    #[test]
    fn event_names_do_not_bypass_config() {
        let src = "enum DownloadEvent { Retrying { attempts: u32 } }\n";
        assert_eq!(count_violations_with_allowed(src, &["attempt"]), 1);
    }

    #[test]
    fn still_flags_unconfigured_identifiers() {
        let src = "struct RetryTelemetry { max_retries: u32 }\n";
        assert_eq!(count_violations(src), 1);
    }

    #[test]
    fn allows_unrelated_names() {
        let src = "struct Foo { primary: u8, secondary: u8 }\nfn read() {}\n";
        assert_eq!(count_violations(src), 0);
    }

    #[test]
    fn allows_a_borrowed_policy_configuration_method() {
        assert_eq!(count_violations(CONFIGURATION_SETTER), 0);
    }

    #[test]
    fn configuration_setter_is_independent_of_domain_names() {
        let source = r"
            impl Router {
                fn with_fallback_url(&self, endpoint: Endpoint) -> Self {
                    let mut settings = self.settings().clone();
                    settings.fallback_url = endpoint;
                    self.with_settings(settings)
                }
            }
        ";
        assert_eq!(count_violations(source), 0);
    }

    #[test]
    fn configuration_setter_body_remains_checked() {
        let source = CONFIGURATION_SETTER
            .replace("let mut options", "let mut fallback_settings")
            .replace("options.retry_policy", "fallback_settings.retry_policy")
            .replace("with_options(options)", "with_options(fallback_settings)");
        assert_eq!(count_violations(&source), 1);
        let counter = CONFIGURATION_SETTER.replace(
            "options.retry_policy = retry_policy;",
            "let attempts = 0; options.retry_policy = retry_policy;",
        );
        assert_eq!(count_violations(&counter), 2);
    }

    #[test]
    fn configuration_setter_requires_borrowed_synchronous_value_contract() {
        for (original, replacement) in [
            ("&self", "self"),
            ("&self", "&mut self"),
            ("RetryPolicy)", "&RetryPolicy)"),
            ("RetryPolicy)", "RetryPolicy, enabled: bool)"),
            ("-> Self", "-> Result<Self>"),
            ("pub fn", "pub async fn"),
            ("pub fn", "pub unsafe fn"),
            ("with_retry_policy(", "with_retry_policy<T>("),
        ] {
            let source = CONFIGURATION_SETTER.replace(original, replacement);
            assert!(count_violations(&source) > 0, "{replacement}");
        }
    }

    #[test]
    fn configuration_setter_rejects_execution_logic() {
        for (original, replacement) in [
            (
                "options.retry_policy = retry_policy;",
                "if ready { options.retry_policy = retry_policy; }",
            ),
            (
                "options.retry_policy = retry_policy;",
                "loop { options.retry_policy = retry_policy; break; }",
            ),
            (
                "options.retry_policy = retry_policy;",
                "let observed = 0; options.retry_policy = retry_policy;",
            ),
            ("self.options().clone()", "self.options().await.clone()"),
            (
                "options.retry_policy = retry_policy;",
                "self.execute(); options.retry_policy = retry_policy;",
            ),
            ("self.options().clone()", "self.options(true).clone()"),
            ("self.options().clone()", "self.fallback_options().clone()"),
            ("self.with_options(options)", "self.with_retry(options)"),
            ("self.with_options(options)", "self.execute(options)"),
            (
                "options.retry_policy = retry_policy",
                "options.retry_policy = make_policy()",
            ),
            (
                "self.with_options(options)",
                "self.with_options(options, retry_policy)",
            ),
        ] {
            let source = CONFIGURATION_SETTER.replace(original, replacement);
            assert_eq!(count_violations(&source), 1, "{replacement}");
        }
    }

    #[test]
    fn suppression_silences_violation() {
        let src = "// xtask-lint-ignore: retry_fallback\nstruct Req { attempt: u8 }\n";
        assert_eq!(count_violations(src), 0);
    }
}
