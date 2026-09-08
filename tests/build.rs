fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../crates/kithara-test-fixtures/cache-version");
    println!(
        "cargo:rustc-env=KITHARA_FIXTURE_BUILD={}",
        include_str!("../crates/kithara-test-fixtures/cache-version").trim()
    );
}
