mod encoding;
mod operations;
/// UI label = the variant name (`Signalsmith` / `Bungee`), via `Debug`, so
/// the selector needs no per-variant `cfg` arm.
#[cfg(test)]
mod tests;
mod value;

pub use value::{BackendCapabilities, StretchKind};
