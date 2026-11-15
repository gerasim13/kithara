use kithara_test_args::TestArgs;
use syn::{Expr, parse_quote};

pub(crate) fn resolve(_args: &TestArgs) -> Expr {
    parse_quote!(::std::time::Duration::from_secs(120))
}
