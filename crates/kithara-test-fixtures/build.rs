#[cfg(feature = "generate")]
include!("build_generator.rs");

#[cfg(feature = "generate")]
fn main() {
    run();
}

#[cfg(not(feature = "generate"))]
fn main() {}
