// The committed catalog lets normal test consumers read a prepared fixture
// store without compiling the encoder graph. The producer builds this crate
// with `generate`, which keeps wasm's embedded fixtures and optional-asset
// diagnostics in the build output.
#[cfg(feature = "generate")]
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

#[cfg(not(feature = "generate"))]
include!("catalog.rs");
