use syn::{Block, ExprForLoop, visit::Visit};

pub(super) fn body_has_jump(body: &Block) -> bool {
    let mut visitor = JumpFinder { found: false };
    visitor.visit_block(body);
    visitor.found
}

struct JumpFinder {
    found: bool,
}

impl<'ast> Visit<'ast> for JumpFinder {
    fn visit_expr_break(&mut self, _: &'ast syn::ExprBreak) {
        self.found = true;
    }
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_continue(&mut self, _: &'ast syn::ExprContinue) {
        self.found = true;
    }
    fn visit_expr_for_loop(&mut self, _: &'ast ExprForLoop) {}
    fn visit_expr_loop(&mut self, _: &'ast syn::ExprLoop) {}
    fn visit_expr_return(&mut self, _: &'ast syn::ExprReturn) {
        self.found = true;
    }
    fn visit_expr_while(&mut self, _: &'ast syn::ExprWhile) {}
}
