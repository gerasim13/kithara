//! Prepares portable signals in `OUT_DIR` and opt-in native assets in the store.

mod portable;
mod portable_contract;

fn main() {
    println!("cargo::rerun-if-changed=build/main.rs");
    println!("cargo::rerun-if-changed=build/portable.rs");
    println!("cargo::rerun-if-changed=build/portable_contract.rs");
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    portable::generate(&output);
    portable_contract::verify(&output);
    #[cfg(feature = "native-fixtures")]
    kithara_fixture_gen::generate();
}
