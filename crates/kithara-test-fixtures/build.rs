//! Materializes every registered asset into the shared store and writes one
//! accessor per case into `OUT_DIR`. The definitions and the generator live in
//! `kithara-fixture-gen`, a build dependency only, which is what keeps encoding
//! itself out of the target build.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    #[cfg(feature = "native-fixtures")]
    kithara_fixture_gen::generate();
}
