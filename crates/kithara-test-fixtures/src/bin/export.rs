//! Writes one generated body to a path, for builds that cannot link the crate.
//!
//! The store is content-addressed under a build fingerprint, so a foreign build
//! system has no stable path to read. Gradle packages the exported file into
//! the instrumentation APK.

use std::{fs, path::PathBuf, process::ExitCode};

use kithara_test_fixtures::assets::by_name;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(name), Some(out)) = (args.next(), args.next().map(PathBuf::from)) else {
        eprintln!("usage: kithara-fixture-export <accessor-name> <output-path>");
        return ExitCode::FAILURE;
    };
    let name = name.to_string_lossy().into_owned();

    let Some(asset) = by_name(&name) else {
        eprintln!("kithara-fixture-export: no generated asset is named `{name}`");
        return ExitCode::FAILURE;
    };

    if let Some(parent) = out.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!(
            "kithara-fixture-export: create {}: {error}",
            parent.display()
        );
        return ExitCode::FAILURE;
    }
    if let Err(error) = fs::write(&out, asset.bytes()) {
        eprintln!("kithara-fixture-export: write {}: {error}", out.display());
        return ExitCode::FAILURE;
    }

    println!("kithara-fixture-export: {name} -> {}", out.display());
    ExitCode::SUCCESS
}
