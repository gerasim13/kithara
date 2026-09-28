//! Body facts: every call site with the decision arms enclosing it, the arms
//! themselves with their line span and inline tokens, and the local bindings
//! whose types the resolver infers receivers from.

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use quote::ToTokens;
use serde::Serialize;
use syn::{
    Expr, Item, Member, Pat, Stmt, Token,
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
};

use super::facts::{Import, cfgs_of, idents_of, last_ident, path_segs, use_tree};

mod consts {
    /// Methods that hand their receiver's type through unchanged.
    pub(super) const TRANSPARENT: &[&str] = &[
        "lock",
        "read",
        "write",
        "borrow",
        "borrow_mut",
        "as_ref",
        "as_mut",
        "as_deref",
        "as_deref_mut",
        "unwrap",
        "expect",
        "clone",
        "get_mut",
        "deref",
        "deref_mut",
        "load",
        "load_full",
        "upgrade",
        "iter",
        "iter_mut",
        "into_iter",
        "blocking_lock",
        "try_lock",
        "get",
        "first",
        "last",
        "values",
        "values_mut",
        "as_slice",
        "take",
        "lock_sync",
        "unwrap_or_default",
    ];
    /// Methods whose arguments run only when the receiver holds no value.
    pub(super) const ALTERNATIVE: &[&str] = &[
        "or_else",
        "unwrap_or_else",
        "or",
        "unwrap_or",
        "map_or_else",
        "map_or",
    ];
    pub(super) const DIVERGING_MACROS: &[&str] =
        &["panic", "unreachable", "bail", "todo", "unimplemented"];
    /// Words that mark a condition as a failure test.
    pub(super) const FAILURE_WORDS: &[&str] = &[
        "Err",
        "None",
        "is_err",
        "is_none",
        "!is_ok",
        "!is_some",
        "fail",
        "error",
        "Error",
        "invalid",
        "missing",
        "unsupported",
        "Unsupported",
    ];
    /// Lines an arm spans before its own code counts toward its side.
    pub(super) const INLINE_ARM_LINES: usize = 5;
}

/// The construct that splits execution into arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DecisionKind {
    If,
    Match,
    Guard,
    LetElse,
    OrElse,
    Select,
    ShortCircuit,
    Table,
}

impl DecisionKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::If => "if",
            Self::Match => "match",
            Self::Guard => "guard",
            Self::LetElse => "let-else",
            Self::OrElse => "or-else",
            Self::Select => "select",
            Self::ShortCircuit => "short-circuit",
            Self::Table => "table",
        }
    }
}

/// A receiver or initializer expression reduced to what type inference reads.
#[derive(Clone, Debug)]
pub(super) enum Desc {
    SelfValue,
    /// A local binding: an index into the body's locals.
    Var(usize),
    Path(Vec<String>),
    Field {
        of: Box<Self>,
        field: String,
    },
    Ret {
        of: Box<Self>,
        method: String,
    },
    CallRet {
        path: Vec<String>,
        arg: Option<Box<Self>>,
    },
    Tuple(Vec<Self>),
    Ty(Vec<String>),
    Payload {
        of: Box<Self>,
        variant: Vec<String>,
        index: String,
    },
    Unknown,
}

impl Desc {
    fn payload(of: &Self, variant: &[String], index: String) -> Self {
        Self::Payload {
            index,
            of: Box::new(of.clone()),
            variant: variant.to_vec(),
        }
    }
}

/// What a local binding's type is known from.
#[derive(Debug)]
pub(super) enum Local {
    Ty(Vec<String>),
    From(Desc),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SiteKind {
    Method,
    Call,
    Ref,
    Variant,
    New,
}

/// A place in a body that may reach another function.
#[derive(Debug)]
pub(super) struct Site {
    pub(super) recv: Option<Desc>,
    pub(super) kind: SiteKind,
    /// Indices into the body's arms, outermost first.
    pub(super) frames: Vec<usize>,
    pub(super) path: Vec<String>,
    pub(super) line: usize,
}

/// One arm of a decision.
#[derive(Debug)]
pub(super) struct Arm {
    pub(super) lines: (usize, usize),
    pub(super) kind: DecisionKind,
    pub(super) tokens: Option<Vec<String>>,
    pub(super) label: String,
    pub(super) cfg: Vec<String>,
    pub(super) variants: Vec<Vec<String>>,
    pub(super) fail: bool,
    pub(super) decision: u32,
}

#[derive(Debug, Default)]
pub(super) struct BodyFacts {
    pub(super) arms: Vec<Arm>,
    /// Every binding of the body in source order; a shadowing binding is a
    /// new one.
    pub(super) locals: Vec<Local>,
    pub(super) sites: Vec<Site>,
    pub(super) uses: Vec<Import>,
}

#[derive(Default)]
pub(super) struct Body {
    facts: BodyFacts,
    /// Names in scope, innermost last, with their index into the locals.
    scope: Vec<(String, usize)>,
    stack: Vec<usize>,
    decisions: u32,
}

impl Body {
    fn alternative(&mut self, call: &syn::ExprMethodCall, name: &str, recv: &Desc) {
        let decision = self.decision();
        self.push(
            decision,
            DecisionKind::OrElse,
            "primary",
            false,
            lines_of(call.receiver.span()),
        );
        self.visit_expr(&call.receiver);
        self.stack.pop();
        let mapped = name.starts_with("map_or");
        for (index, arg) in call.args.iter().enumerate() {
            let primary = mapped && index == 1;
            let label = if primary { "primary" } else { "alt" };
            self.push(
                decision,
                DecisionKind::OrElse,
                label,
                !primary,
                lines_of(arg.span()),
            );
            self.visit_arg(arg, recv);
            self.stack.pop();
        }
    }

    /// Keeps the inline code of an arm that spans a few lines.
    fn arm_tokens(&mut self, arm: usize, tokens: TokenStream) {
        if let Some(arm) = self.facts.arms.get_mut(arm)
            && arm.lines.1 + 1 >= arm.lines.0 + consts::INLINE_ARM_LINES
        {
            arm.tokens = Some(norm_tokens(tokens));
        }
    }

    fn bind(&mut self, name: String, local: Local) {
        self.scope.push((name, self.facts.locals.len()));
        self.facts.locals.push(local);
    }

    /// A parameter's pattern takes its declared type.
    pub(super) fn bind_param(&mut self, pat: &Pat, ty: Vec<String>) {
        match pat {
            Pat::Ident(ident) => self.bind(ident.ident.to_string(), Local::Ty(ty)),
            _ => self.bind_pat(pat, &Desc::Ty(ty)),
        }
    }

    /// Every identifier a pattern binds takes its type from the destructured
    /// value.
    fn bind_pat(&mut self, pat: &Pat, from: &Desc) {
        match pat {
            Pat::Ident(ident) => {
                if let Some((_, sub)) = &ident.subpat {
                    self.bind_pat(sub, from);
                }
                self.bind(ident.ident.to_string(), Local::From(from.clone()));
            }
            Pat::Type(typed) => {
                if let Pat::Ident(ident) = typed.pat.as_ref() {
                    self.bind(ident.ident.to_string(), Local::Ty(idents_of(&typed.ty)));
                } else {
                    self.bind_pat(&typed.pat, from);
                }
            }
            Pat::TupleStruct(tuple) => {
                let variant = path_segs(&tuple.path);
                for (index, elem) in tuple.elems.iter().enumerate() {
                    self.bind_pat(elem, &Desc::payload(from, &variant, index.to_string()));
                }
            }
            Pat::Tuple(tuple) => {
                let parts = match from {
                    Desc::Tuple(parts) if parts.len() == tuple.elems.len() => Some(parts),
                    _ => None,
                };
                for (index, elem) in tuple.elems.iter().enumerate() {
                    let part = parts.and_then(|parts| parts.get(index)).unwrap_or(from);
                    self.bind_pat(elem, part);
                }
            }
            Pat::Struct(strukt) => {
                let variant = path_segs(&strukt.path);
                for field in &strukt.fields {
                    let member = member_name(&field.member);
                    self.bind_pat(&field.pat, &Desc::payload(from, &variant, member));
                }
            }
            Pat::Reference(reference) => self.bind_pat(&reference.pat, from),
            Pat::Guard(guarded) => self.bind_pat(&guarded.pat, from),
            Pat::Paren(paren) => self.bind_pat(&paren.pat, from),
            Pat::Or(or) => {
                for case in &or.cases {
                    self.bind_pat(case, from);
                }
            }
            Pat::Slice(slice) => {
                for elem in &slice.elems {
                    self.bind_pat(elem, from);
                }
            }
            _ => {}
        }
    }

    /// A closure's parameters bind in its own scope; a closure handed to a
    /// method takes the receiver's type for them.
    fn closure(&mut self, closure: &syn::ExprClosure, from: &Desc) {
        let mark = self.scope.len();
        for input in &closure.inputs {
            self.visit_pat(input);
            self.bind_pat(input, from);
        }
        self.visit_expr(&closure.body);
        self.scope.truncate(mark);
    }

    fn decision(&mut self) -> u32 {
        self.decisions += 1;
        self.decisions
    }

    fn describe(&self, expr: &Expr) -> Desc {
        match expr {
            Expr::Path(path) if path.path.segments.len() == 1 && path.qself.is_none() => {
                let name = path_segs(&path.path).concat();
                if name == "self" {
                    Desc::SelfValue
                } else {
                    self.local(&name).map_or(Desc::Unknown, Desc::Var)
                }
            }
            Expr::Path(path) => Desc::Path(path_segs(&path.path)),
            Expr::Field(field) => Desc::Field {
                of: Box::new(self.describe(&field.base)),
                field: member_name(&field.member),
            },
            Expr::MethodCall(call) => {
                let method = call.method.to_string();
                if consts::TRANSPARENT.contains(&method.as_str()) {
                    self.describe(&call.receiver)
                } else {
                    Desc::Ret {
                        method,
                        of: Box::new(self.describe(&call.receiver)),
                    }
                }
            }
            Expr::Call(call) => match call.func.as_ref() {
                Expr::Path(path) => Desc::CallRet {
                    path: path_segs(&path.path),
                    arg: call.args.first().map(|arg| Box::new(self.describe(arg))),
                },
                _ => Desc::Unknown,
            },
            Expr::Tuple(tuple) => {
                Desc::Tuple(tuple.elems.iter().map(|elem| self.describe(elem)).collect())
            }
            Expr::Struct(strukt) => Desc::CallRet {
                path: path_segs(&strukt.path),
                arg: None,
            },
            Expr::Paren(paren) => self.describe(&paren.expr),
            Expr::Group(group) => self.describe(&group.expr),
            Expr::Reference(reference) => self.describe(&reference.expr),
            Expr::Unary(unary) => self.describe(&unary.expr),
            Expr::Try(tried) => self.describe(&tried.expr),
            Expr::Await(awaited) => self.describe(&awaited.base),
            Expr::Index(index) => self.describe(&index.expr),
            Expr::Cast(cast) => Desc::Ty(idents_of(&cast.ty)),
            _ => Desc::Unknown,
        }
    }

    pub(super) fn finish(self) -> BodyFacts {
        self.facts
    }

    fn guard(&mut self, ifx: &syn::ExprIf, tail: Tail<'_>) {
        let mark = self.scope.len();
        self.visit_expr(&ifx.cond);
        let decision = self.decision();
        let fail = failure_text(&ifx.cond.to_token_stream().to_string());
        let arm = self.push(
            decision,
            DecisionKind::Guard,
            "then",
            fail,
            lines_of(ifx.then_branch.span()),
        );
        self.arm_tokens(arm, ifx.then_branch.to_token_stream());
        self.visit_block(&ifx.then_branch);
        self.stack.pop();
        self.scope.truncate(mark);
        let arm = self.push(decision, DecisionKind::Guard, "rest", false, tail.lines);
        self.arm_tokens(arm, tail.tokens());
    }

    pub(super) fn has_sites(&self) -> bool {
        !self.facts.sites.is_empty()
    }

    fn let_else(&mut self, local: &syn::Local, init: &Expr, diverge: &Expr, tail: Tail<'_>) {
        let from = self.describe(init);
        self.visit_expr(init);
        self.visit_pat(&local.pat);
        self.bind_pat(&local.pat, &from);
        let decision = self.decision();
        let arm = self.push(
            decision,
            DecisionKind::LetElse,
            "else",
            true,
            lines_of(diverge.span()),
        );
        self.arm_tokens(arm, diverge.to_token_stream());
        self.visit_expr(diverge);
        self.stack.pop();
        let arm = self.push(decision, DecisionKind::LetElse, "rest", false, tail.lines);
        self.arm_tokens(arm, tail.tokens());
    }

    fn local(&self, name: &str) -> Option<usize> {
        self.scope
            .iter()
            .rev()
            .find(|(bound, _)| bound == name)
            .map(|(_, id)| *id)
    }

    fn push(
        &mut self,
        decision: u32,
        kind: DecisionKind,
        label: &str,
        fail: bool,
        lines: (usize, usize),
    ) -> usize {
        let index = self.facts.arms.len();
        self.facts.arms.push(Arm {
            decision,
            kind,
            fail,
            lines,
            label: label.to_string(),
            variants: Vec::new(),
            cfg: Vec::new(),
            tokens: None,
        });
        self.stack.push(index);
        index
    }

    /// Call sites inside macro or attribute tokens that do not parse as
    /// expressions: `a::b(..)` is a call, `.m(..)` a method, `A::B` a variant.
    pub(super) fn scan_tokens(&mut self, tokens: TokenStream, line: usize) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let mut index = 0;
        while let Some(tree) = trees.get(index) {
            match tree {
                TokenTree::Group(group) => self.scan_tokens(group.stream(), line),
                TokenTree::Ident(ident) => {
                    let mut path = vec![ident.to_string()];
                    let mut next = index + 1;
                    while is_colon(trees.get(next))
                        && is_colon(trees.get(next + 1))
                        && let Some(TokenTree::Ident(segment)) = trees.get(next + 2)
                    {
                        path.push(segment.to_string());
                        next += 3;
                    }
                    self.scanned_path(&trees, index, next, path, line);
                    index = next;
                    continue;
                }
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
            index += 1;
        }
    }

    fn scanned_path(
        &mut self,
        trees: &[TokenTree],
        start: usize,
        end: usize,
        path: Vec<String>,
        line: usize,
    ) {
        let called = matches!(
            trees.get(end),
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis
        );
        let after_dot = start > 0 && is_punct(trees.get(start - 1), '.');
        if called && after_dot {
            let recv = start
                .checked_sub(2)
                .map(|at| self.scanned_receiver(trees, at));
            self.site(SiteKind::Method, path, recv, line);
        } else if called {
            self.site(SiteKind::Call, path, None, line);
        } else if path.len() >= 2 && path.last().is_some_and(|last| is_upper(last)) {
            self.site(SiteKind::Variant, path, None, line);
        }
    }

    /// The receiver in front of `.method(..)` in raw tokens: `self`, `self.field`
    /// or a variable.
    fn scanned_receiver(&self, trees: &[TokenTree], at: usize) -> Desc {
        match trees.get(at) {
            Some(TokenTree::Ident(recv)) if recv == "self" => Desc::SelfValue,
            Some(TokenTree::Ident(recv))
                if at >= 2
                    && is_punct(trees.get(at - 1), '.')
                    && matches!(trees.get(at - 2), Some(TokenTree::Ident(s)) if s == "self") =>
            {
                Desc::Field {
                    of: Box::new(Desc::SelfValue),
                    field: recv.to_string(),
                }
            }
            Some(TokenTree::Ident(recv)) => self
                .local(&recv.to_string())
                .map_or(Desc::Unknown, Desc::Var),
            _ => Desc::Unknown,
        }
    }

    fn select(&mut self, tokens: TokenStream, line: usize) {
        let decision = self.decision();
        let mut arms = vec![TokenStream::new()];
        for tree in tokens {
            if matches!(&tree, TokenTree::Punct(p) if p.as_char() == ',') {
                arms.push(TokenStream::new());
            } else if let Some(arm) = arms.last_mut() {
                arm.extend([tree]);
            }
        }
        for (index, arm) in arms.into_iter().enumerate() {
            self.push(
                decision,
                DecisionKind::Select,
                &index.to_string(),
                false,
                (line, line),
            );
            self.scan_tokens(arm, line);
            self.stack.pop();
        }
    }

    fn site(&mut self, kind: SiteKind, path: Vec<String>, recv: Option<Desc>, line: usize) {
        self.facts.sites.push(Site {
            kind,
            path,
            recv,
            line,
            frames: self.stack.clone(),
        });
    }

    fn visit_arg(&mut self, arg: &Expr, recv: &Desc) {
        if let Expr::Closure(closure) = arg {
            self.closure(closure, recv);
        } else {
            self.visit_expr(arg);
        }
    }

    /// `[f, g]` or `vec![f, g]` of plain paths or closures is a strategy
    /// table: each entry is an arm.
    fn visit_table(&mut self, elems: &[&Expr]) -> bool {
        if elems.len() < 2
            || !elems
                .iter()
                .all(|e| matches!(e, Expr::Path(_) | Expr::Closure(_)))
        {
            return false;
        }
        let decision = self.decision();
        for (index, elem) in elems.iter().enumerate() {
            self.push(
                decision,
                DecisionKind::Table,
                &index.to_string(),
                false,
                lines_of(elem.span()),
            );
            self.visit_expr(elem);
            self.stack.pop();
        }
        true
    }
}

/// The statements after a split statement: the arm that runs when the split
/// does not leave the block.
#[derive(Clone, Copy)]
struct Tail<'a> {
    stmts: &'a [Stmt],
    lines: (usize, usize),
}

impl Tail<'_> {
    fn tokens(self) -> TokenStream {
        self.stmts.iter().map(ToTokens::to_token_stream).collect()
    }
}

impl<'ast> Visit<'ast> for Body {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let mark = self.scope.len();
        let mut open = 0;
        let end = block.span().end().line;
        for (index, stmt) in block.stmts.iter().enumerate() {
            let tail = Tail {
                lines: (stmt.span().end().line + 1, end),
                stmts: block.stmts.get(index + 1..).unwrap_or_default(),
            };
            if let Some((local, init, diverge)) = let_else_parts(stmt) {
                self.let_else(local, init, diverge, tail);
                open += 1;
            } else if let Stmt::Expr(Expr::If(ifx), _) = stmt
                && ifx.else_branch.is_none()
                && diverges(&ifx.then_branch)
            {
                self.guard(ifx, tail);
                open += 1;
            } else {
                self.visit_stmt(stmt);
            }
        }
        for _ in 0..open {
            self.stack.pop();
        }
        self.scope.truncate(mark);
    }

    fn visit_expr_array(&mut self, array: &'ast syn::ExprArray) {
        let elems: Vec<&Expr> = array.elems.iter().collect();
        if !self.visit_table(&elems) {
            visit::visit_expr_array(self, array);
        }
    }

    /// `a || b` runs `b` only when `a` fails; `a && b` only when it holds.
    fn visit_expr_binary(&mut self, binary: &'ast syn::ExprBinary) {
        let or = matches!(binary.op, syn::BinOp::Or(_));
        if !or && !matches!(binary.op, syn::BinOp::And(_)) {
            visit::visit_expr_binary(self, binary);
            return;
        }
        let decision = self.decision();
        self.push(
            decision,
            DecisionKind::ShortCircuit,
            "lhs",
            false,
            lines_of(binary.left.span()),
        );
        self.visit_expr(&binary.left);
        self.stack.pop();
        self.push(
            decision,
            DecisionKind::ShortCircuit,
            "rhs",
            or,
            lines_of(binary.right.span()),
        );
        self.visit_expr(&binary.right);
        self.stack.pop();
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(path) = call.func.as_ref() {
            let mut segs = path_segs(&path.path);
            if let Some(ty) = path.qself.as_ref().and_then(|q| last_ident(&q.ty)) {
                segs.insert(0, ty);
            }
            self.site(SiteKind::Call, segs, None, path.path.span().start().line);
        } else {
            self.visit_expr(&call.func);
        }
        for arg in &call.args {
            self.visit_expr(arg);
        }
    }

    fn visit_expr_closure(&mut self, closure: &'ast syn::ExprClosure) {
        self.closure(closure, &Desc::Unknown);
    }

    fn visit_expr_for_loop(&mut self, looped: &'ast syn::ExprForLoop) {
        let from = self.describe(&looped.expr);
        self.visit_expr(&looped.expr);
        let mark = self.scope.len();
        self.visit_pat(&looped.pat);
        self.bind_pat(&looped.pat, &from);
        self.visit_block(&looped.body);
        self.scope.truncate(mark);
    }

    /// An `else if` ladder is one decision with one arm per rung.
    fn visit_expr_if(&mut self, ifx: &'ast syn::ExprIf) {
        let mark = self.scope.len();
        self.visit_expr(&ifx.cond);
        let decision = self.decision();
        let fail = failure_text(&ifx.cond.to_token_stream().to_string());
        let arm = self.push(
            decision,
            DecisionKind::If,
            "then",
            fail,
            lines_of(ifx.then_branch.span()),
        );
        self.arm_tokens(arm, ifx.then_branch.to_token_stream());
        self.visit_block(&ifx.then_branch);
        self.stack.pop();
        self.scope.truncate(mark);
        let mut next = ifx.else_branch.as_ref().map(|(_, e)| e.as_ref());
        let mut rung = 0;
        while let Some(branch) = next {
            if let Expr::If(elif) = branch {
                rung += 1;
                let arm = self.push(
                    decision,
                    DecisionKind::If,
                    &format!("elif{rung}"),
                    false,
                    lines_of(elif.then_branch.span()),
                );
                self.arm_tokens(arm, elif.then_branch.to_token_stream());
                self.visit_expr(&elif.cond);
                self.visit_block(&elif.then_branch);
                self.stack.pop();
                self.scope.truncate(mark);
                next = elif.else_branch.as_ref().map(|(_, e)| e.as_ref());
            } else {
                let arm = self.push(
                    decision,
                    DecisionKind::If,
                    "else",
                    false,
                    lines_of(branch.span()),
                );
                self.arm_tokens(arm, branch.to_token_stream());
                self.visit_expr(branch);
                self.stack.pop();
                next = None;
            }
        }
    }

    fn visit_expr_let(&mut self, binding: &'ast syn::ExprLet) {
        let from = self.describe(&binding.expr);
        self.visit_expr(&binding.expr);
        self.visit_pat(&binding.pat);
        self.bind_pat(&binding.pat, &from);
    }

    fn visit_expr_match(&mut self, matched: &'ast syn::ExprMatch) {
        self.visit_expr(&matched.expr);
        let decision = self.decision();
        let scrutinee = self.describe(&matched.expr);
        for (index, arm) in matched.arms.iter().enumerate() {
            let mark = self.scope.len();
            self.bind_pat(&arm.pat, &scrutinee);
            let mut variants = Vec::new();
            pat_variants(&arm.pat, &mut variants);
            let fail = variants
                .iter()
                .any(|v| matches!(v.last().map(String::as_str), Some("Err" | "None")));
            let at = self.push(
                decision,
                DecisionKind::Match,
                &index.to_string(),
                fail,
                lines_of(arm.span()),
            );
            self.arm_tokens(at, arm.body.to_token_stream());
            if let Some(pushed) = self.facts.arms.get_mut(at) {
                pushed.variants = variants;
                pushed.cfg = cfgs_of(&arm.attrs);
            }
            self.visit_pat(&arm.pat);
            self.visit_expr(&arm.body);
            self.stack.pop();
            self.scope.truncate(mark);
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        let recv = self.describe(&call.receiver);
        let line = call.method.span().start().line;
        self.site(
            SiteKind::Method,
            vec![name.clone()],
            Some(recv.clone()),
            line,
        );
        if consts::ALTERNATIVE.contains(&name.as_str()) {
            self.alternative(call, &name, &recv);
        } else {
            self.visit_expr(&call.receiver);
            for arg in &call.args {
                self.visit_arg(arg, &recv);
            }
        }
    }

    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        let segs = path_segs(&path.path);
        let line = path.path.span().start().line;
        let last = segs.last().cloned().unwrap_or_default();
        let upper = is_upper(&last);
        if segs.len() == 1 {
            if !upper && self.local(&last).is_none() && last != "self" {
                self.site(SiteKind::Ref, segs, None, line);
            }
        } else if upper {
            self.site(SiteKind::Variant, segs, None, line);
        } else {
            self.site(SiteKind::Ref, segs, None, line);
        }
    }

    fn visit_expr_struct(&mut self, strukt: &'ast syn::ExprStruct) {
        let segs = path_segs(&strukt.path);
        let variant = segs.len() >= 2 && segs.get(segs.len() - 2).is_some_and(|s| is_upper(s));
        let kind = if variant {
            SiteKind::Variant
        } else {
            SiteKind::New
        };
        self.site(kind, segs, None, strukt.path.span().start().line);
        for field in &strukt.fields {
            self.visit_expr(&field.expr);
        }
        if let Some(rest) = &strukt.rest {
            self.visit_expr(rest);
        }
    }

    fn visit_item(&mut self, item: &'ast Item) {
        if let Item::Use(import) = item {
            use_tree(&import.tree, &mut Vec::new(), &mut self.facts.uses);
        }
    }

    /// The initializer still sees the bindings the new one shadows.
    fn visit_local(&mut self, local: &'ast syn::Local) {
        let from = local
            .init
            .as_ref()
            .map_or(Desc::Unknown, |init| self.describe(&init.expr));
        if let Some(init) = &local.init {
            self.visit_local_init(init);
        }
        self.visit_pat(&local.pat);
        self.bind_pat(&local.pat, &from);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let line = mac.path.span().start().line;
        if let Ok(exprs) = mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
            let elems: Vec<&Expr> = exprs.iter().collect();
            if name == "vec" && self.visit_table(&elems) {
                return;
            }
            for expr in &exprs {
                self.visit_expr(expr);
            }
        } else if name.contains("select") {
            self.select(mac.tokens.clone(), line);
        } else {
            self.scan_tokens(mac.tokens.clone(), line);
        }
    }
}

pub(super) fn lines_of(span: Span) -> (usize, usize) {
    (span.start().line, span.end().line)
}

fn is_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

fn is_punct(tree: Option<&TokenTree>, ch: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

fn is_colon(tree: Option<&TokenTree>) -> bool {
    is_punct(tree, ':')
}

fn member_name(member: &Member) -> String {
    match member {
        Member::Named(ident) => ident.to_string(),
        Member::Unnamed(index) => index.index.to_string(),
    }
}

fn pat_variants(pat: &Pat, out: &mut Vec<Vec<String>>) {
    match pat {
        Pat::TupleStruct(tuple) => {
            out.push(path_segs(&tuple.path));
            for elem in &tuple.elems {
                pat_variants(elem, out);
            }
        }
        Pat::Struct(strukt) => {
            out.push(path_segs(&strukt.path));
            for field in &strukt.fields {
                pat_variants(&field.pat, out);
            }
        }
        Pat::Path(path) => out.push(path_segs(&path.path)),
        Pat::Or(or) => {
            for case in &or.cases {
                pat_variants(case, out);
            }
        }
        Pat::Ident(ident) => {
            if let Some((_, sub)) = &ident.subpat {
                pat_variants(sub, out);
            } else if is_upper(&ident.ident.to_string()) {
                out.push(vec![ident.ident.to_string()]);
            }
        }
        Pat::Reference(reference) => pat_variants(&reference.pat, out),
        Pat::Guard(guarded) => pat_variants(&guarded.pat, out),
        Pat::Paren(paren) => pat_variants(&paren.pat, out),
        Pat::Tuple(tuple) => {
            for elem in &tuple.elems {
                pat_variants(elem, out);
            }
        }
        Pat::Slice(slice) => {
            for elem in &slice.elems {
                pat_variants(elem, out);
            }
        }
        _ => {}
    }
}

fn let_else_parts(stmt: &Stmt) -> Option<(&syn::Local, &Expr, &Expr)> {
    let Stmt::Local(local) = stmt else {
        return None;
    };
    let init = local.init.as_ref()?;
    let (_, diverge) = init.diverge.as_ref()?;
    Some((local, &init.expr, diverge))
}

fn failure_text(text: &str) -> bool {
    let text = text.replace(' ', "");
    consts::FAILURE_WORDS.iter().any(|word| text.contains(word))
}

fn diverging_macro(mac: &syn::Macro) -> bool {
    mac.path
        .segments
        .last()
        .is_some_and(|s| consts::DIVERGING_MACROS.contains(&s.ident.to_string().as_str()))
}

fn diverges(block: &syn::Block) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Stmt::Expr(Expr::Return(_) | Expr::Break(_) | Expr::Continue(_), _) => true,
        Stmt::Expr(Expr::Macro(mac), _) => diverging_macro(&mac.mac),
        Stmt::Macro(mac) => diverging_macro(&mac.mac),
        _ => false,
    })
}

/// Tokens with literals folded to `S` (text) or `N` (number) and groups kept
/// as their delimiters.
pub(super) fn norm_tokens(tokens: TokenStream) -> Vec<String> {
    let mut out = Vec::new();
    push_tokens(tokens, &mut out);
    out
}

fn push_tokens(tokens: TokenStream, out: &mut Vec<String>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) => out.push(ident.to_string()),
            TokenTree::Punct(punct) => out.push(punct.as_char().to_string()),
            TokenTree::Literal(literal) => {
                let text = literal.to_string();
                let textual =
                    text.starts_with('"') || text.starts_with('r') || text.starts_with('b');
                out.push(if textual { "S" } else { "N" }.to_string());
            }
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                if !open.is_empty() {
                    out.push(open.to_string());
                }
                push_tokens(group.stream(), out);
                if !close.is_empty() {
                    out.push(close.to_string());
                }
            }
        }
    }
}
