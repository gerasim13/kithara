use kithara_test_args::TestArgs;
use syn::{Expr, parse_quote};

pub(crate) fn resolve(_args: &TestArgs) -> Expr {
    parse_quote!(::std::time::Duration::from_secs(120))
}

#[cfg(test)]
mod tests {
    use kithara_test_args::TestArgs;
    use quote::ToTokens;
    use syn::Expr;

    use super::resolve;

    #[test]
    fn a_missing_timeout_uses_a_constant_quick_budget() -> syn::Result<()> {
        let args = syn::parse_str::<TestArgs>("")?;
        let expanded = resolve(&args).to_token_stream().to_string();
        assert!(expanded.contains("const __TIMEOUT"));
        assert!(expanded.contains("TestBudget :: Quick . timeout"));
        assert!(expanded.contains("kithara_platform :: flash :: ENABLED"));
        Ok(())
    }

    #[test]
    fn named_budgets_keep_the_explicit_clock_and_compiled_feature() -> syn::Result<()> {
        let args = syn::parse_str::<TestArgs>("flash(false), timeout(Medium)")?;
        let expanded = resolve(&args).to_token_stream().to_string();
        assert!(expanded.contains("TestBudget :: Medium . timeout"));
        assert!(expanded.contains("timeout (false)"));
        assert!(!expanded.contains("flash :: ENABLED"));
        Ok(())
    }

    #[test]
    fn manual_expressions_are_replaced_only_in_miri() -> syn::Result<()> {
        let args = syn::parse_str::<TestArgs>("timeout(Duration::from_secs(5))")?;
        let expr = resolve(&args);
        let Expr::Block(block) = &expr else {
            panic!("manual timeout must select its mode at compile time");
        };
        assert_eq!(block.block.stmts.len(), 2);
        let expanded = expr.to_token_stream().to_string();
        assert!(expanded.contains("cfg (miri)"));
        assert!(expanded.contains("TestBudget :: Long . timeout"));
        assert!(expanded.contains("cfg (not (miri))"));
        assert!(expanded.contains("Duration :: from_secs (5)"));
        Ok(())
    }
}
