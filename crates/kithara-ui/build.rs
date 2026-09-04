//! Embed the shipped asset folder, so the library that answers for a document
//! and the folder that holds it cannot drift apart.
//!
//! A hand-written table went stale the moment a document was added without an
//! entry: every consumer then patched the gap with a copy of its own. The
//! folder is the one place a document is written, so the table is read from it.
//!
//! Only what a document names is embedded here. Fonts, icons, lottie artwork
//! and shaders are named by code rather than by a document, and each has its
//! own owner in `src/`.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn main() {
    let assets = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest"))
        .join("assets");
    println!("cargo:rerun-if-changed={}", assets.display());

    let mut files = Vec::new();
    collect(&assets, &assets, &mut files);
    files.sort();

    let mut code = String::from("const ASSETS: &[(&str, &str)] = &[\n");
    for (path, file) in files.iter().filter(|(path, _)| path.ends_with(".ron")) {
        writeln!(code, "    ({path:?}, include_str!({file:?})),").expect("a String never fails");
    }
    code.push_str("];\nconst PICTURES: &[(&str, &[u8])] = &[\n");
    for (path, file) in files
        .iter()
        .filter(|(path, _)| path.starts_with("sprites/"))
    {
        writeln!(code, "    ({path:?}, include_bytes!({file:?})),").expect("a String never fails");
    }
    code.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets the output directory"))
        .join("builtin_assets.rs");
    fs::write(&out, code).unwrap_or_else(|error| panic!("write {}: {error}", out.display()));
}

/// Every file under `dir`, as the path a document names it by and the path the
/// build reads it from.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
            .path();
        if path.is_dir() {
            collect(root, &path, out);
            continue;
        }
        let named = path
            .strip_prefix(root)
            .expect("every file walked sits under the folder")
            .to_str()
            .expect("the shipped folder holds no unnameable path")
            .replace('\\', "/");
        let read = path
            .to_str()
            .expect("the checkout holds no unnameable path")
            .to_owned();
        out.push((named, read));
    }
}
