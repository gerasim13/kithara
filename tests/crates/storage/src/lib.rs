#![forbid(unsafe_code)]

pub use kithara::bufpool::testing as bufpool_ext;

#[path = "../../../src/fixtures.rs"]
mod fixtures;
#[path = "../../../src/storage_ext.rs"]
pub mod storage_ext;

pub use fixtures::*;
