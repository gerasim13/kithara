#![forbid(unsafe_code)]

//! Integration tests for kithara-stream

use kithara_test_dylib as _;

#[path = "../../../src/memory_source.rs"]
mod memory_source;
#[cfg(not(target_arch = "wasm32"))]
mod reader_seek_overflow;
mod source;
mod sync_reader_basic_test;
