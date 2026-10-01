#![forbid(unsafe_code)]

mod curves;
// Exhaustive 2^32-pattern validation runs in the normal test lanes.
#[cfg(not(coverage))]
mod sanitize;
